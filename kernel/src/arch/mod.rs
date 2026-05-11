pub mod x86_64;
use crate::serial_println;

/// Perform all architecture-level initialisation.
///
/// Order matters:
/// 1. Hardware security (NXE, SMEP, SMAP) – must be first so that no
///    insecure window exists.
/// 2. GDT + TSS – required before the IDT can reference the IST stacks.
/// 3. IDT – installs exception handlers so CPU faults don't triple-fault.
pub fn init() {
    self::x86_64::security::harden();
    self::x86_64::gdt::init();
    self::x86_64::idt::init();
    serial_println!("arch: GDT/TSS/IDT loaded");
}
