//! Local APIC + I/O APIC driver.
//!
//! The x86-64 architecture provides two interrupt controllers:
//!   * **Local APIC** (LAPIC) — one per CPU core, handles inter-processor
//!     interrupts (IPI), APIC timer, and accepts interrupts from the I/O APIC.
//!   * **I/O APIC** — a separate chip that receives external hardware IRQs
//!     (keyboard, disk, etc.) and routes them to one or more LAPIC(s).
//!
//! For now we:
//!   1. Detect the LAPIC MMIO base from IA32_APIC_BASE MSR.
//!   2. Enable the LAPIC (set bit 8 of the spurious vector register).
//!   3. Map the well-known I/O APIC at 0xFEC0_0000 and redirect IRQ 0 (PIT)
//!      and IRQ 1 (keyboard) to the same vectors as the 8259 PIC (0x20, 0x21)
//!      so the rest of the kernel works unchanged.
//!   4. Keep the 8259 PIC disabled (both masks = 0xFF) when the APIC is active.
//!
//! In a QEMU/VirtualBox `-machine q35` or `-machine pc` environment ACPI
//! tables would give us the exact I/O APIC base; we hard-code the standard
//! address (0xFEC0_0000) used by virtually every x86 platform since Pentium Pro.

use core::ptr::{read_volatile, write_volatile};

use x86_64::registers::model_specific::Msr;
use x86_64::PhysAddr;

use crate::serial_println;

// ---------------------------------------------------------------------------
// Local APIC register offsets (relative to LAPIC MMIO base, 32-bit aligned)
// ---------------------------------------------------------------------------
const LAPIC_ID: u32 = 0x020;
const LAPIC_VER: u32 = 0x030;
const LAPIC_TPR: u32 = 0x080; // Task Priority Register — set to 0 to accept all
const LAPIC_SVR: u32 = 0x0F0; // Spurious Vector Register
const LAPIC_EOI: u32 = 0x0B0; // End-Of-Interrupt

const LAPIC_SVR_ENABLE: u32 = 1 << 8;
const LAPIC_SPURIOUS_VECTOR: u32 = 0xFF; // vector 255 for spurious IRQs

// IA32_APIC_BASE MSR
const MSR_APIC_BASE: u32 = 0x1B;
const APIC_BASE_ENABLE: u64 = 1 << 11;
const APIC_BASE_ADDR_MASK: u64 = 0xFFFF_FFFF_F000;

// ---------------------------------------------------------------------------
// I/O APIC register indices (indirect: write index to IOREGSEL, read/write
// data from IOWIN)
// ---------------------------------------------------------------------------
const IOAPIC_BASE_PHYS: u64 = 0xFEC0_0000;
const IOAPIC_REGSEL: u32 = 0x00;
const IOAPIC_WIN: u32 = 0x10;

const IOAPIC_REG_ID: u32 = 0x00;
const IOAPIC_REG_VER: u32 = 0x01;
/// Redirection table base — IRQ n is at 0x10 + 2*n (lo) and 0x10 + 2*n + 1 (hi).
const IOAPIC_REDIR_BASE: u32 = 0x10;

/// Redirection entry bit: masked.
const IOAPIC_REDIR_MASKED: u32 = 1 << 16;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Physical base of the LAPIC MMIO window (set once by `init()`).
/// We cast to a pointer only inside `unsafe` helpers.
static mut LAPIC_BASE: *mut u32 = core::ptr::null_mut();

/// Returns the virtual address of the LAPIC MMIO window.
/// In the bootloader identity-map all physical addresses are mapped 1:1 up to
/// the `physical_memory_offset`; the LAPIC sits below 4 GiB so a direct cast
/// is fine for the initial bring-up.  A proper implementation would go through
/// the physical-memory offset from BootInfo.
unsafe fn lapic_read(reg: u32) -> u32 {
    read_volatile(LAPIC_BASE.byte_add(reg as usize))
}

unsafe fn lapic_write(reg: u32, val: u32) {
    write_volatile(LAPIC_BASE.byte_add(reg as usize), val);
}

/// Send an End-Of-Interrupt signal to the Local APIC.
/// Must be called at the end of every LAPIC-delivered interrupt handler.
pub fn end_of_interrupt() {
    // SAFETY: init() ensures LAPIC_BASE is valid.
    unsafe { lapic_write(LAPIC_EOI, 0) };
}

// I/O APIC helpers — single I/O APIC at the standard address.
unsafe fn ioapic_ptr() -> *mut u32 {
    IOAPIC_BASE_PHYS as *mut u32
}

unsafe fn ioapic_read(reg: u32) -> u32 {
    let base = ioapic_ptr();
    write_volatile(base.byte_add(IOAPIC_REGSEL as usize), reg);
    read_volatile(base.byte_add(IOAPIC_WIN as usize))
}

unsafe fn ioapic_write(reg: u32, val: u32) {
    let base = ioapic_ptr();
    write_volatile(base.byte_add(IOAPIC_REGSEL as usize), reg);
    write_volatile(base.byte_add(IOAPIC_WIN as usize), val);
}

/// Write one 64-bit redirection table entry.
unsafe fn ioapic_set_redir(irq: u8, vector: u8, masked: bool) {
    let lo_idx = IOAPIC_REDIR_BASE + 2 * irq as u32;
    let hi_idx = lo_idx + 1;

    // High word: deliver to APIC ID 0 (BSP).
    ioapic_write(hi_idx, 0x0000_0000);

    // Low word: fixed delivery, physical destination, active-high, edge-triggered.
    let lo = if masked {
        IOAPIC_REDIR_MASKED | vector as u32
    } else {
        vector as u32
    };
    ioapic_write(lo_idx, lo);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the Local APIC and I/O APIC.
///
/// # Safety
/// Must be called after paging is set up.  Writes to MSRs and MMIO.
pub unsafe fn init() {
    // --- Read LAPIC base address from MSR ---
    let mut apic_base_msr = Msr::new(MSR_APIC_BASE);
    let apic_base_val = apic_base_msr.read();

    // Enable the APIC globally (bit 11 of IA32_APIC_BASE).
    apic_base_msr.write(apic_base_val | APIC_BASE_ENABLE);

    let lapic_phys = apic_base_val & APIC_BASE_ADDR_MASK;
    // Direct physical cast — valid because bootloader maps all physical memory.
    LAPIC_BASE = lapic_phys as *mut u32;

    // --- Enable Local APIC via Spurious Vector Register ---
    lapic_write(LAPIC_TPR, 0); // accept all priority levels
    let svr = lapic_read(LAPIC_SVR);
    lapic_write(LAPIC_SVR, svr | LAPIC_SVR_ENABLE | LAPIC_SPURIOUS_VECTOR);

    let lapic_id = lapic_read(LAPIC_ID) >> 24;
    let lapic_ver = lapic_read(LAPIC_VER) & 0xFF;
    serial_println!(
        "apic: LAPIC id={} ver=0x{:02x} base={:#x}",
        lapic_id,
        lapic_ver,
        lapic_phys
    );

    // --- I/O APIC: redirect IRQ 0 (PIT) and IRQ 1 (keyboard) ---
    // Use same vector numbers as the 8259 mapping (0x20 = 32, 0x21 = 33) so
    // the existing IDT handlers continue to work.
    let ioapic_ver = ioapic_read(IOAPIC_REG_VER) & 0xFF;
    let max_redir = (ioapic_read(IOAPIC_REG_VER) >> 16) & 0xFF;
    serial_println!(
        "apic: IOAPIC ver=0x{:02x} max_redir={}",
        ioapic_ver,
        max_redir
    );

    ioapic_set_redir(0, 0x20, false); // IRQ 0 → vector 0x20 (PIT timer)
    ioapic_set_redir(1, 0x21, false); // IRQ 1 → vector 0x21 (PS/2 keyboard)

    // Mask all other I/O APIC IRQs (2–23).
    for irq in 2..=23u8 {
        if (irq as u32) <= max_redir {
            ioapic_set_redir(irq, 0x20 + irq, true);
        }
    }

    serial_println!("apic: Local APIC + I/O APIC initialised");
}

/// Returns the physical address of the Local APIC MMIO window (for debugging).
pub fn lapic_base() -> PhysAddr {
    PhysAddr::new(unsafe { LAPIC_BASE } as u64)
}
