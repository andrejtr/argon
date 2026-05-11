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
/// 4. PIC – remap IRQs to vectors 32–47, mask all but timer + keyboard.
/// 5. PIT – program the timer to fire at 100 Hz.
/// 6. Enable interrupts (sti).
pub fn init() {
    self::x86_64::security::harden();
    self::x86_64::gdt::init();
    self::x86_64::idt::init();

    // SAFETY: called once, after IDT is loaded.
    unsafe {
        self::x86_64::pic::init();
        self::x86_64::pit::init();
    }

    // Enable hardware interrupts now that all handlers are installed.
    interrupts::enable();

    serial_println!("arch: GDT/TSS/IDT/PIC/PIT loaded, interrupts enabled");
}
