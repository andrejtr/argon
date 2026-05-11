//! AHCI (Advanced Host Controller Interface) storage driver.
//!
//! Detects the AHCI HBA via PCI class 0x01 / subclass 0x06, maps its MMIO
//! registers, enables AHCI mode, identifies attached SATA disks, allocates
//! per-port DMA buffers, and registers each disk as a `BlockDevice`.
//!
//! DMA reads use ATA READ DMA EXT (opcode 0x25) with a single command slot
//! and a pre-allocated bounce buffer, polled for completion.

use core::ptr::{read_volatile, write_volatile};

use crate::serial_println;

use super::pci;
use crate::block::{BlockDevice, BlockError};

// PCI class/subclass for AHCI
const PCI_CLASS_STORAGE: u8 = 0x01;
const PCI_SUBCLASS_SATA: u8 = 0x06;

// ---------------------------------------------------------------------------
// HBA MMIO register offsets (relative to ABAR)
// ---------------------------------------------------------------------------
const HBA_CAP: u32 = 0x00; // Host Capabilities
const HBA_GHC: u32 = 0x04; // Global HBA Control
const HBA_IS: u32 = 0x08; // Interrupt Status
const HBA_PI: u32 = 0x0C; // Ports Implemented bitmask
const HBA_VS: u32 = 0x10; // Version

const GHC_AE: u32 = 1 << 31; // AHCI Enable

// Port register offsets (relative to HBA_BASE + 0x100 + port*0x80)
const PORT_CLB: u32 = 0x00; // Command List Base (low)
const PORT_CLBU: u32 = 0x04; // Command List Base (high)
const PORT_FB: u32 = 0x08; // FIS Base (low)
const PORT_FBU: u32 = 0x0C; // FIS Base (high)
const PORT_IS: u32 = 0x10; // Interrupt Status
const PORT_CMD: u32 = 0x18; // Command and Status
const PORT_TFD: u32 = 0x20; // Task File Data
const PORT_SIG: u32 = 0x28; // Signature
const PORT_SSTS: u32 = 0x2C; // Serial ATA Status
const PORT_SERR: u32 = 0x34; // Serial ATA Error
const PORT_CI: u32 = 0x3C; // Command Issue

// Port CMD flags
const PORT_CMD_ST: u32 = 1 << 0; // Start DMA engine
const PORT_CMD_FRE: u32 = 1 << 4; // FIS Receive Enable
const PORT_CMD_FR: u32 = 1 << 14; // FIS Receive Running
const PORT_CMD_CR: u32 = 1 << 15; // Command List Running

// PORT_IS flags
const PORT_IS_TFES: u32 = 1 << 30; // Task File Error Status

// SATA device signatures
const SATA_SIG_ATA: u32 = 0x0000_0101; // SATA hard disk
const SATA_SIG_ATAPI: u32 = 0xEB14_0101; // SATAPI (optical)

// ATA commands / FIS constants
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const FIS_TYPE_H2D: u8 = 0x27;

// Max sectors per DMA transfer (bounce buffer = 1 frame = 4096 bytes = 8 sectors)
const MAX_SECTORS_PER_XFER: u32 = 8;

// ---------------------------------------------------------------------------
// HBA accessor helpers
// ---------------------------------------------------------------------------

static mut HBA_BASE: *mut u32 = core::ptr::null_mut();

unsafe fn hba_read(reg: u32) -> u32 {
    read_volatile(HBA_BASE.byte_add(reg as usize))
}

unsafe fn hba_write(reg: u32, val: u32) {
    write_volatile(HBA_BASE.byte_add(reg as usize), val);
}

unsafe fn port_base(port: u32) -> *mut u32 {
    HBA_BASE.byte_add(0x100 + port as usize * 0x80)
}

unsafe fn port_read(port: u32, reg: u32) -> u32 {
    read_volatile(port_base(port).byte_add(reg as usize))
}

unsafe fn port_write(port: u32, reg: u32, val: u32) {
    write_volatile(port_base(port).byte_add(reg as usize), val);
}

// ---------------------------------------------------------------------------
// Port helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    Ata,
    Atapi,
    None,
}

unsafe fn port_stop(port: u32) {
    let mut cmd = port_read(port, PORT_CMD);
    cmd &= !(PORT_CMD_ST | PORT_CMD_FRE);
    port_write(port, PORT_CMD, cmd);
    for _ in 0..500_000u32 {
        let cmd = port_read(port, PORT_CMD);
        if (cmd & (PORT_CMD_FR | PORT_CMD_CR)) == 0 {
            break;
        }
    }
}

unsafe fn port_start(port: u32) {
    let mut cmd = port_read(port, PORT_CMD);
    cmd |= PORT_CMD_FRE | PORT_CMD_ST;
    port_write(port, PORT_CMD, cmd);
}

unsafe fn port_kind(port: u32) -> DriveKind {
    let ssts = port_read(port, PORT_SSTS);
    let det = ssts & 0xF;
    let ipm = (ssts >> 8) & 0xF;
    if det != 3 || ipm != 1 {
        return DriveKind::None;
    }
    match port_read(port, PORT_SIG) {
        SATA_SIG_ATA => DriveKind::Ata,
        SATA_SIG_ATAPI => DriveKind::Atapi,
        _ => DriveKind::None,
    }
}

// ---------------------------------------------------------------------------
// AHCI block device
// ---------------------------------------------------------------------------

/// One AHCI port with an ATA disk attached.
///
/// All addresses are stored as plain `u64` so the struct satisfies `Send`.
pub struct AhciBlockDevice {
    port: u32,
    /// Virtual address of the command list (slot 0 = first 32 bytes).
    cl_virt: u64,
    /// Physical address of the pre-allocated command table.
    ct_phys: u64,
    /// Virtual address of the pre-allocated command table.
    ct_virt: u64,
    /// Physical address of the DMA bounce buffer (4 KiB = 8 sectors).
    bb_phys: u64,
    /// Virtual address of the DMA bounce buffer.
    bb_virt: u64,
}

// SAFETY: Raw addresses for MMIO/DMA memory exclusively managed by this
// driver.  Concurrent access is serialised by the block-layer DEVICES mutex.
unsafe impl Send for AhciBlockDevice {}

impl AhciBlockDevice {
    /// Issue one polled DMA READ DMA EXT for up to MAX_SECTORS_PER_XFER sectors.
    unsafe fn do_read(&self, lba: u64, sectors: u16, dst: &mut [u8]) -> bool {
        let cl_ptr = self.cl_virt as *mut u32;
        let cfis = self.ct_virt as *mut u8;
        let prdt = (self.ct_virt + 128) as *mut u32;
        let bb = self.bb_virt as *mut u8;
        let chunk_bytes = sectors as usize * 512;

        // ── Command header (slot 0) ──
        // DW0: PRDTL=1 | W=0 (device→host) | CFL=5 (H2D FIS = 5 DWORDs)
        write_volatile(cl_ptr.add(0), (1u32 << 16) | 5);
        write_volatile(cl_ptr.add(1), 0); // PRDBC – written by HBA
        write_volatile(cl_ptr.add(2), self.ct_phys as u32);
        write_volatile(cl_ptr.add(3), (self.ct_phys >> 32) as u32);
        for i in 4..8usize {
            write_volatile(cl_ptr.add(i), 0);
        }

        // ── H2D Register FIS (CFIS area at command-table offset 0) ──
        core::ptr::write_bytes(cfis, 0, 64);
        write_volatile(cfis.add(0), FIS_TYPE_H2D);
        write_volatile(cfis.add(1), 0x80u8); // C=1 (command update)
        write_volatile(cfis.add(2), ATA_CMD_READ_DMA_EXT);
        write_volatile(cfis.add(3), 0u8); // features low
        write_volatile(cfis.add(4), lba as u8);
        write_volatile(cfis.add(5), (lba >> 8) as u8);
        write_volatile(cfis.add(6), (lba >> 16) as u8);
        write_volatile(cfis.add(7), 0x40u8); // device: LBA mode
        write_volatile(cfis.add(8), (lba >> 24) as u8);
        write_volatile(cfis.add(9), (lba >> 32) as u8);
        write_volatile(cfis.add(10), (lba >> 40) as u8);
        write_volatile(cfis.add(11), 0u8); // features high
        write_volatile(cfis.add(12), sectors as u8);
        write_volatile(cfis.add(13), (sectors >> 8) as u8);
        // bytes 14-19 (ICC / control / aux) are already zero

        // ── PRDT entry 0 (command-table offset 128) ──
        write_volatile(prdt.add(0), self.bb_phys as u32); // DBA low
        write_volatile(prdt.add(1), (self.bb_phys >> 32) as u32); // DBA high
        write_volatile(prdt.add(2), 0u32);
        // DBC = byte_count - 1; bit 31 = interrupt on completion
        write_volatile(prdt.add(3), ((chunk_bytes as u32) - 1) | (1 << 31));

        // ── Submit ──
        port_write(self.port, PORT_IS, 0xFFFF_FFFF);
        port_write(self.port, PORT_SERR, 0xFFFF_FFFF);
        port_write(self.port, PORT_CI, 1); // issue slot 0

        // ── Poll for completion ──
        let mut ok = false;
        for _ in 0u32..10_000_000 {
            let is_val = port_read(self.port, PORT_IS);
            if is_val & PORT_IS_TFES != 0 {
                serial_println!(
                    "ahci: port {} DMA error IS={:#x} TFD={:#x}",
                    self.port,
                    is_val,
                    port_read(self.port, PORT_TFD)
                );
                return false;
            }
            if port_read(self.port, PORT_CI) & 1 == 0 {
                ok = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !ok {
            serial_println!(
                "ahci: port {} DMA timeout lba={} sectors={}",
                self.port,
                lba,
                sectors
            );
            return false;
        }

        // Copy bounce buffer → caller's slice
        core::ptr::copy_nonoverlapping(bb, dst.as_mut_ptr(), chunk_bytes);
        true
    }
}

impl BlockDevice for AhciBlockDevice {
    fn read_blocks(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
        assert_eq!(buf.len(), count as usize * 512, "buf size mismatch");
        let mut remaining = count;
        let mut cur_lba = lba;
        let mut off = 0usize;
        while remaining > 0 {
            let chunk = remaining.min(MAX_SECTORS_PER_XFER) as u16;
            let cb = chunk as usize * 512;
            let ok = unsafe { self.do_read(cur_lba, chunk, &mut buf[off..off + cb]) };
            if !ok {
                return Err(BlockError::Io);
            }
            remaining -= chunk as u32;
            cur_lba += chunk as u64;
            off += cb;
        }
        Ok(())
    }

    fn block_count(&self) -> u64 {
        u64::MAX // placeholder; real value requires IDENTIFY DEVICE
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Detect and initialise the AHCI HBA, then register each ATA port in the
/// global block-device registry.
///
/// # Safety
/// Writes to MMIO registers.  Must be called after the memory subsystem
/// (physical-memory offset) is initialised.
pub unsafe fn init() {
    let dev = match pci::find_device(PCI_CLASS_STORAGE, PCI_SUBCLASS_SATA) {
        Some(d) => d,
        None => {
            serial_println!("ahci: no SATA controller found");
            return;
        }
    };

    let abar = dev.bar(5);
    if abar == 0 {
        serial_println!("ahci: ABAR is zero – skipping");
        return;
    }
    serial_println!(
        "ahci: HBA at PCI {:02x}:{:02x}.{} ABAR={:#x}",
        dev.bus,
        dev.device,
        dev.function,
        abar
    );

    dev.enable_bus_master();

    HBA_BASE = crate::memory::phys_to_virt(x86_64::PhysAddr::new(abar)).as_mut_ptr();

    // Enable AHCI mode.
    let ghc = hba_read(HBA_GHC);
    if ghc & GHC_AE == 0 {
        hba_write(HBA_GHC, ghc | GHC_AE);
    }

    let version = hba_read(HBA_VS);
    let cap = hba_read(HBA_CAP);
    let num_ports = (cap & 0x1F) + 1;
    serial_println!(
        "ahci: AHCI v{}.{}.{} cap={:#x} ports={}",
        (version >> 16) & 0xFF,
        (version >> 8) & 0xFF,
        version & 0xFF,
        cap,
        num_ports
    );

    let pi = hba_read(HBA_PI);
    hba_write(HBA_IS, hba_read(HBA_IS)); // clear global IS

    for port in 0..32u32 {
        if pi & (1 << port) == 0 {
            continue;
        }
        port_write(port, PORT_SERR, 0xFFFF_FFFF);
        port_write(port, PORT_IS, 0xFFFF_FFFF);

        let kind = port_kind(port);
        if kind != DriveKind::Ata {
            continue;
        }

        // Allocate per-port DMA buffers: command list, FIS buffer,
        // command table, and bounce buffer (each one 4 KiB frame).
        let cl_frame = crate::memory::frame_alloc::alloc_frame().expect("ahci: OOM command list");
        let fis_frame = crate::memory::frame_alloc::alloc_frame().expect("ahci: OOM FIS buffer");
        let ct_frame = crate::memory::frame_alloc::alloc_frame().expect("ahci: OOM command table");
        let bb_frame = crate::memory::frame_alloc::alloc_frame().expect("ahci: OOM bounce buffer");

        let cl_virt = crate::memory::phys_to_virt(cl_frame.start_address());
        let fis_virt = crate::memory::phys_to_virt(fis_frame.start_address());
        let ct_virt = crate::memory::phys_to_virt(ct_frame.start_address());
        let bb_virt = crate::memory::phys_to_virt(bb_frame.start_address());

        core::ptr::write_bytes(cl_virt.as_mut_ptr::<u8>(), 0, 4096);
        core::ptr::write_bytes(fis_virt.as_mut_ptr::<u8>(), 0, 4096);
        core::ptr::write_bytes(ct_virt.as_mut_ptr::<u8>(), 0, 4096);
        core::ptr::write_bytes(bb_virt.as_mut_ptr::<u8>(), 0, 4096);

        port_stop(port);

        let cl_phys = cl_frame.start_address().as_u64();
        let fis_phys = fis_frame.start_address().as_u64();
        let ct_phys = ct_frame.start_address().as_u64();
        let bb_phys = bb_frame.start_address().as_u64();

        port_write(port, PORT_CLB, cl_phys as u32);
        port_write(port, PORT_CLBU, (cl_phys >> 32) as u32);
        port_write(port, PORT_FB, fis_phys as u32);
        port_write(port, PORT_FBU, (fis_phys >> 32) as u32);

        port_start(port);

        serial_println!(
            "ahci: port {} ATA cl={:#x} ct={:#x} bb={:#x}",
            port,
            cl_phys,
            ct_phys,
            bb_phys
        );

        crate::block::register(alloc::boxed::Box::new(AhciBlockDevice {
            port,
            cl_virt: cl_virt.as_u64(),
            ct_phys,
            ct_virt: ct_virt.as_u64(),
            bb_phys,
            bb_virt: bb_virt.as_u64(),
        }));
    }

    serial_println!("ahci: initialised ({} disks)", crate::block::device_count());
}
