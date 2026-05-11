#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod arch;
mod display;
mod future;
mod memory;
mod panic;
mod serial;

use bootloader_api::{entry_point, BootInfo};
use future::ramfs::RamFs;
use future::vfs::FileSystem;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. Architecture: security hardening, GDT, IDT, PIC, PIT, sti.
    arch::init();

    // 2. Serial — COM1 debug output.
    serial::init();
    serial_println!("argonOS booting...");

    // 3. Memory: page table + heap (must come before any alloc).
    let phys_mem_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("bootloader must provide physical_memory_offset");
    memory::init(phys_mem_offset);
    serial_println!("memory: OK  (heap 1 MiB)");

    // 4. Boot splash.
    display::init(boot_info);

    // 5. RamFS smoke-test.
    ramfs_demo();

    // 6. Scheduler: spawn tasks and start round-robin.
    future::scheduler::init();
    future::scheduler::spawn(kernel_task_a);
    future::scheduler::spawn(kernel_task_b);
    serial_println!("scheduler: running");

    serial_println!("argonOS ready.");

    // Idle loop — the scheduler switches away from here on each timer tick.
    loop {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

// ---------------------------------------------------------------------------
// Demo kernel tasks
// ---------------------------------------------------------------------------

fn kernel_task_a() -> ! {
    let mut n = 0u64;
    loop {
        if n.is_multiple_of(500) {
            serial_println!("task-A: tick {}", n);
        }
        n += 1;
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

fn kernel_task_b() -> ! {
    let mut n = 0u64;
    loop {
        if n.is_multiple_of(500) {
            serial_println!("task-B: tick {}", n);
        }
        n += 1;
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

// ---------------------------------------------------------------------------
// RamFS demo
// ---------------------------------------------------------------------------

fn ramfs_demo() {
    let mut fs = RamFs::new();

    fs.create("/etc/os-release", b"NAME=argonOS\nVERSION=1.0.0.0\n")
        .expect("ramfs create");
    fs.create("/boot/motd", b"Welcome to argonOS!\n")
        .expect("ramfs create");

    let fd = fs.open("/etc/os-release").expect("ramfs open");
    let mut buf = [0u8; 64];
    let n = fs.read(fd, &mut buf).expect("ramfs read");
    fs.close(fd).expect("ramfs close");

    let content = core::str::from_utf8(&buf[..n]).unwrap_or("<utf8 error>");
    serial_println!("ramfs: /etc/os-release = {:?}", content);
    serial_println!("ramfs: OK");
}
