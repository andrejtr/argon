//! NVMe (Non-Volatile Memory Express) controller driver.
//!
//! Detects the NVMe controller via PCI class 0x01 / subclass 0x08, maps the
//! BAR0 MMIO register region, sets up the admin submission/completion queue
//! pair, enables the controller and submits an Identify Namespace command.
//!
//! I/O queues and command dispatch are left as future work; this
//! implementation demonstrates controller enumeration and IDENTIFY.

use core::ptr::{read_volatile, write_volatile};

use crate::serial_println;

use super::pci;

// PCI class/subclass for NVMe
const PCI_CLASS_STORAGE: u8 = 0x01;
const PCI_SUBCLASS_NVME: u8 = 0x08;

// ---------------------------------------------------------------------------
// NVMe Controller Register Offsets (BAR0)
// ---------------------------------------------------------------------------
const NVME_CAP: u32 = 0x00; // Controller Capabilities (64-bit)
const NVME_VS: u32 = 0x08; // Version
const NVME_AQA: u32 = 0x24; // Admin Queue Attributes
const NVME_ASQ: u32 = 0x28; // Admin Submission Queue Base (64-bit)
const NVME_ACQ: u32 = 0x30; // Admin Completion Queue Base (64-bit)
const NVME_CC: u32 = 0x14; // Controller Configuration
const NVME_CSTS: u32 = 0x1C; // Controller Status

// CC bits
const CC_EN: u32 = 1 << 0;
const CC_CSS_NVM: u32 = 0 << 4; // Command Set: NVM
const CC_MPS_4K: u32 = 0 << 7; // Memory Page Size: 4 KiB (MPS field = 0 → 2^(12+0))
const CC_IOSQES: u32 = 6 << 16; // Submission queue entry size = 2^6 = 64 B
const CC_IOCQES: u32 = 4 << 20; // Completion queue entry size = 2^4 = 16 B

// CSTS bits
const CSTS_RDY: u32 = 1 << 0;
const CSTS_CFS: u32 = 1 << 1; // Controller Fatal Status

/// Number of entries in admin queues (must be ≤ 4096).
const ADMIN_QUEUE_DEPTH: u32 = 64;

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

unsafe fn nvme_read32(base: *mut u8, offset: u32) -> u32 {
    read_volatile(base.byte_add(offset as usize) as *mut u32)
}

unsafe fn nvme_write32(base: *mut u8, offset: u32, val: u32) {
    write_volatile(base.byte_add(offset as usize) as *mut u32, val);
}

unsafe fn nvme_read64(base: *mut u8, offset: u32) -> u64 {
    let lo = read_volatile(base.byte_add(offset as usize) as *mut u32) as u64;
    let hi = read_volatile(base.byte_add(offset as usize + 4) as *mut u32) as u64;
    lo | (hi << 32)
}

unsafe fn nvme_write64(base: *mut u8, offset: u32, val: u64) {
    write_volatile(base.byte_add(offset as usize) as *mut u32, val as u32);
    write_volatile(
        base.byte_add(offset as usize + 4) as *mut u32,
        (val >> 32) as u32,
    );
}

// ---------------------------------------------------------------------------
// NVMe submission queue entry (64 bytes)
// ---------------------------------------------------------------------------
#[repr(C)]
struct SqEntry {
    cdw0: u32, // Command DWORD 0: opcode, fuse, CID
    nsid: u32, // Namespace ID
    cdw2: u32,
    cdw3: u32,
    mptr: u64,      // Metadata Pointer
    dptr: [u64; 2], // Data Pointer (PRP or SGL)
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
}

// ---------------------------------------------------------------------------
// NVMe completion queue entry (16 bytes)
// ---------------------------------------------------------------------------
#[repr(C)]
struct CqEntry {
    dw0: u32,
    dw1: u32,
    sq_head: u16,
    sq_id: u16,
    cid: u16,
    status: u16, // bit 0 = phase tag
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

pub unsafe fn init() {
    let dev = match pci::find_device(PCI_CLASS_STORAGE, PCI_SUBCLASS_NVME) {
        Some(d) => d,
        None => {
            serial_println!("nvme: no controller found");
            return;
        }
    };

    let bar0 = dev.bar(0);
    if bar0 == 0 {
        serial_println!("nvme: BAR0 is zero – skipping");
        return;
    }
    serial_println!(
        "nvme: controller at PCI {:02x}:{:02x}.{} BAR0={:#x}",
        dev.bus,
        dev.device,
        dev.function,
        bar0
    );

    dev.enable_bus_master();
    let base: *mut u8 = crate::memory::phys_to_virt(x86_64::PhysAddr::new(bar0)).as_mut_ptr();

    // Reset: clear CC.EN, wait for CSTS.RDY to clear.
    nvme_write32(base, NVME_CC, 0);
    for _ in 0..1_000_000u32 {
        if nvme_read32(base, NVME_CSTS) & CSTS_RDY == 0 {
            break;
        }
    }
    if nvme_read32(base, NVME_CSTS) & CSTS_RDY != 0 {
        serial_println!("nvme: controller did not go idle after reset");
        return;
    }

    let cap = nvme_read64(base, NVME_CAP);
    let version = nvme_read32(base, NVME_VS);
    serial_println!(
        "nvme: CAP={:#x} v{}.{}",
        cap,
        (version >> 16) & 0xFFFF,
        (version >> 8) & 0xFF
    );

    // Allocate admin submission queue (ADMIN_QUEUE_DEPTH × 64 B = 4096 B = 1 frame).
    let asq_frame = crate::memory::frame_alloc::alloc_frame().expect("nvme: OOM for admin SQ");
    let acq_frame = crate::memory::frame_alloc::alloc_frame().expect("nvme: OOM for admin CQ");
    // Allocate one data page for Identify responses.
    let data_frame =
        crate::memory::frame_alloc::alloc_frame().expect("nvme: OOM for identify data");

    let asq_virt = crate::memory::phys_to_virt(asq_frame.start_address());
    let acq_virt = crate::memory::phys_to_virt(acq_frame.start_address());
    core::ptr::write_bytes(asq_virt.as_mut_ptr::<u8>(), 0, 4096);
    core::ptr::write_bytes(acq_virt.as_mut_ptr::<u8>(), 0, 4096);
    core::ptr::write_bytes(
        crate::memory::phys_to_virt(data_frame.start_address()).as_mut_ptr::<u8>(),
        0,
        4096,
    );

    let asq_phys = asq_frame.start_address().as_u64();
    let acq_phys = acq_frame.start_address().as_u64();
    let data_phys = data_frame.start_address().as_u64();

    // AQA: admin SQ size - 1 and CQ size - 1.
    let depth = ADMIN_QUEUE_DEPTH - 1;
    nvme_write32(base, NVME_AQA, (depth << 16) | depth);
    nvme_write64(base, NVME_ASQ, asq_phys);
    nvme_write64(base, NVME_ACQ, acq_phys);

    // Enable controller.
    let cc = CC_EN | CC_CSS_NVM | CC_MPS_4K | CC_IOSQES | CC_IOCQES;
    nvme_write32(base, NVME_CC, cc);

    // Wait for CSTS.RDY.
    let mut ready = false;
    for _ in 0..2_000_000u32 {
        let csts = nvme_read32(base, NVME_CSTS);
        if csts & CSTS_CFS != 0 {
            serial_println!("nvme: controller fatal status during enable");
            return;
        }
        if csts & CSTS_RDY != 0 {
            ready = true;
            break;
        }
    }
    if !ready {
        serial_println!("nvme: controller did not become ready");
        return;
    }

    // Submit Identify Controller command (opcode 0x06, CNS=1) via admin SQ slot 0.
    let sqe = &mut *(asq_virt.as_mut_ptr::<SqEntry>());
    sqe.cdw0 = 0x06 | (1 << 16); // opcode=0x06, CID=1
    sqe.nsid = 0;
    sqe.mptr = 0;
    sqe.dptr = [data_phys, 0];
    sqe.cdw10 = 1; // CNS = 1 (Identify Controller)
    sqe.cdw11 = 0;
    sqe.cdw12 = 0;
    sqe.cdw13 = 0;
    sqe.cdw14 = 0;
    sqe.cdw15 = 0;

    // Doorbell: admin SQ tail = 1 (offset = CAP.DSTRD * 4 + 0x1000).
    // ASQ tail doorbell is at 0x1000 (queue 0, submission = even index).
    let dstrd = ((cap >> 32) & 0xF) as u32;
    let sq_doorbell = 0x1000u32 + 2 * dstrd; // ASQ tail doorbell (queue 0)
    nvme_write32(base, sq_doorbell, 1);

    // Poll CQ phase bit (initial phase = 1 after enable).
    let cqe = &*(acq_virt.as_ptr::<CqEntry>());
    let mut done = false;
    for _ in 0..1_000_000u32 {
        let status = read_volatile(&cqe.status);
        if status & 1 != 0 {
            done = true;
            break;
        }
    }
    if !done {
        serial_println!("nvme: identify timed out");
        return;
    }

    // Parse model number bytes 24–63 from Identify Controller data.
    let id_data = crate::memory::phys_to_virt(data_frame.start_address());
    let model_bytes = core::slice::from_raw_parts(id_data.as_ptr::<u8>().add(24), 40);
    // Trim trailing spaces.
    let model_len = model_bytes
        .iter()
        .rposition(|&b| b != b' ')
        .map(|p| p + 1)
        .unwrap_or(0);
    let model = core::str::from_utf8(&model_bytes[..model_len]).unwrap_or("<utf8 err>");
    serial_println!("nvme: Identify OK model=\"{}\"", model);

    // Acknowledge CQ entry: advance CQ 0 head doorbell.
    // Doorbell for queue N, type T (0=SQ,1=CQ) = 0x1000 + (2N+T) * (4 << DSTRD).
    // For admin CQ (N=0, T=1): offset = 0x1000 + (4 << DSTRD).
    let cq_doorbell = 0x1000u32 + (4u32 << dstrd);
    nvme_write32(base, cq_doorbell, 1);

    serial_println!("nvme: initialised");
}
