use core::panic::PanicInfo;
use crate::serial_println;

/// Kernel panic handler.
///
/// Attempts to print the panic location over serial before halting.
/// We deliberately avoid any heap or complex state here because the kernel
/// may be in an inconsistent state when this is called.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("KERNEL PANIC: {}", info);
    halt_loop()
}

/// Disable interrupts and halt the CPU permanently.
#[inline(always)]
pub fn halt_loop() -> ! {
    loop {
        x86_64::instructions::interrupts::disable();
        x86_64::instructions::hlt();
    }
}
