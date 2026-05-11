pub mod x86_64;
use crate::serial_println;
use ::x86_64::instructions::interrupts;

/// Perform all architecture-level initialisation.
///
/// Order matters:
/// 1. Hardware security (NXE, SMEP, SMAP) – must be first so that no
///    insecure window exists.
/// 2. GDT + TSS – required before the IDT can reference the IST stacks.
/// 3. IDT – installs exception handlers so CPU faults don't triple-fault.
/// 4. PerCPU – GS-base scratch area for SYSCALL stack switching.
/// 5. SYSCALL/SYSRET MSRs – fast syscall entry point.
/// 6. PIC – remap IRQs to vectors 32–47, mask all but timer + keyboard.
/// 7. PIT – program the timer to fire at 100 Hz.
/// 8. APIC – Local APIC + IOAPIC (supplements / will replace PIC).
/// 9. Enable interrupts (sti).
pub fn init() {
    self::x86_64::security::harden();
    self::x86_64::gdt::init();
    self::x86_64::idt::init();

    // SAFETY: called once, after GDT/IDT are loaded.
    unsafe {
        self::x86_64::percpu::init();
        self::x86_64::syscall::init();
        self::x86_64::pic::init();
        self::x86_64::pit::init();
        self::x86_64::apic::init();
    }

    // Enable hardware interrupts now that all handlers are installed.
    interrupts::enable();

    serial_println!("arch: GDT/IDT/PERCPU/SYSCALL/PIC/PIT/APIC loaded, interrupts enabled");
}
