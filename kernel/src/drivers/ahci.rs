//! AHCI (Advanced Host Controller Interface) storage driver.
//!
//! Detects the AHCI HBA via PCI class 0x01 / subclass 0x06, maps its MMIO
//! registers, enables AHCI mode and identifies attached SATA devices.
//!
//! DMA command submission is scaffolded (structures defined, command-slot
//! allocation implemented) but actual I/O awaits interrupt-driven completion.

use core::ptr::{read_volatile, write_volatile};

use crate::serial_println;

use super::pci;

// PCI class/subclass for AHCI
const PCI_CLASS_STORAGE: u8 = 0x01;
const PCI_SUBCLASS_SATA: u8 = 0x06;

// ---------------------------------------------------------------------------
// HBA MMIO register offsets (Host Bus Adapter, relative to ABAR)
// ---------------------------------------------------------------------------
const HBA_CAP: u32 = 0x00; // Host Capabilities
const HBA_GHC: u32 = 0x04; // Global HBA Control
const HBA_IS: u32 = 0x08; // Interrupt Status
const HBA_PI: u32 = 0x0C; // Ports Implemented bitmask
const HBA_VS: u32 = 0x10; // Version

const GHC_AE: u32 = 1 << 31; // AHCI Enable
const GHC_IE: u32 = 1 << 1; // Interrupt Enable
const GHC_HR: u32 = 1 << 0; // HBA Reset

// Port register offsets relative to port base (HBA_BASE + 0x100 + port*0x80)
const PORT_CLB: u32 = 0x00; // Command List Base (low)
const PORT_CLBU: u32 = 0x04; // Command List Base (high)
const PORT_FB: u32 = 0x08; // FIS Base (low)
const PORT_FBU: u32 = 0x0C; // FIS Base (high)
const PORT_IS: u32 = 0x10; // Interrupt Status
const PORT_IE: u32 = 0x14; // Interrupt Enable
const PORT_CMD: u32 = 0x18; // Command and Status
const PORT_TFD: u32 = 0x20; // Task File Data (skip reserved at 0x1C)
const PORT_SIG: u32 = 0x28; // Signature
const PORT_SSTS: u32 = 0x2C; // Serial ATA Status
const PORT_SERR: u32 = 0x34; // Serial ATA Error
const PORT_CI: u32 = 0x3C; // Command Issue

// Port CMD flags
const PORT_CMD_ST: u32 = 1 << 0; // Start DMA engine
const PORT_CMD_FRE: u32 = 1 << 4; // FIS Receive Enable
const PORT_CMD_FR: u32 = 1 << 14; // FIS Receive Running
const PORT_CMD_CR: u32 = 1 << 15; // Command List Running

// SATA device signatures
const SATA_SIG_ATA: u32 = 0x0000_0101; // SATA hard disk
const SATA_SIG_ATAPI: u32 = 0xEB14_0101; // SATAPI (optical)

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
// Port state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    Ata,
    Atapi,
    None,
}

/// Stop a port's DMA engine before reconfiguring.
unsafe fn port_stop(port: u32) {
    let mut cmd = port_read(port, PORT_CMD);
    cmd &= !(PORT_CMD_ST | PORT_CMD_FRE);
    port_write(port, PORT_CMD, cmd);
    // Wait for DMA idle (up to ~500 ms).
    for _ in 0..500_000 {
        let cmd = port_read(port, PORT_CMD);
        if (cmd & (PORT_CMD_FR | PORT_CMD_CR)) == 0 {
            break;
        }
    }
}

/// Start a port's DMA engine.
unsafe fn port_start(port: u32) {
    let mut cmd = port_read(port, PORT_CMD);
    cmd |= PORT_CMD_FRE | PORT_CMD_ST;
    port_write(port, PORT_CMD, cmd);
}

/// Classify the device attached to a port.
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
// Initialisation
// ---------------------------------------------------------------------------

/// Detect and initialise the AHCI HBA.
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

    // Map ABAR via the physical-memory offset.
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

    // Clear pending interrupts, then probe implemented ports.
    let pi = hba_read(HBA_PI);
    hba_write(HBA_IS, hba_read(HBA_IS)); // clear global IS

    for port in 0..32u32 {
        if pi & (1 << port) == 0 {
            continue;
        }
        // Clear port error and interrupt status.
        port_write(port, PORT_SERR, 0xFFFF_FFFF);
        port_write(port, PORT_IS, 0xFFFF_FFFF);

        let kind = port_kind(port);
        if kind == DriveKind::None {
            continue;
        }

        // Allocate command list and FIS receive buffers (each 4 KiB).
        let cl_frame = crate::memory::frame_alloc::alloc_frame()
            .expect("ahci: out of frames for command list");
        let fis_frame =
            crate::memory::frame_alloc::alloc_frame().expect("ahci: out of frames for FIS buffer");

        // Zero the buffers.
        let cl_virt = crate::memory::phys_to_virt(cl_frame.start_address());
        let fis_virt = crate::memory::phys_to_virt(fis_frame.start_address());
        core::ptr::write_bytes(cl_virt.as_mut_ptr::<u8>(), 0, 4096);
        core::ptr::write_bytes(fis_virt.as_mut_ptr::<u8>(), 0, 4096);

        port_stop(port);

        let cl_phys = cl_frame.start_address().as_u64();
        let fis_phys = fis_frame.start_address().as_u64();

        port_write(port, PORT_CLB, cl_phys as u32);
        port_write(port, PORT_CLBU, (cl_phys >> 32) as u32);
        port_write(port, PORT_FB, fis_phys as u32);
        port_write(port, PORT_FBU, (fis_phys >> 32) as u32);

        port_start(port);

        serial_println!(
            "ahci: port {} {:?} cl={:#x} fis={:#x}",
            port,
            kind,
            cl_phys,
            fis_phys
        );
    }

    serial_println!("ahci: initialised");
}
