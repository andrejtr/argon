#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

mod arch;
mod display;
mod future;
mod memory;
mod panic;
mod serial;

use bootloader_api::{entry_point, BootInfo};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // Initialise architecture-level structures first (GDT, IDT, security).
    // This must happen before anything that could fault.
    arch::init();

    // Announce ourselves on the serial port (visible in VirtualBox serial log).
    serial::init();
    serial_println!("argonOS booting...");

    // Extract the physical memory offset (Copy u64) before any 'static borrows.
    let phys_mem_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("bootloader must provide physical_memory_offset");

    // Set up physical memory management using just the offset value.
    memory::init(phys_mem_offset);
    serial_println!("memory: OK");

    // Render the boot splash – takes the rest of boot_info (framebuffer etc).
    display::init(boot_info);

    serial_println!("argonOS ready.");

    // Main idle loop – halt the CPU between interrupts to save power.
    loop {
        x86_64::instructions::hlt();
    }
}
