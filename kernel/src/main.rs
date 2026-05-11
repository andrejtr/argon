#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod arch;
mod block;
mod display;
mod drivers;
mod fs;
mod future;
mod memory;
mod panic;
mod serial;
mod smp;

use bootloader_api::{entry_point, BootInfo};
use future::ramfs::RamFs;
use future::vfs::VFS;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // 1. Architecture: security hardening, GDT, IDT, PIC, PIT, sti.
    arch::init();

    // 2. Serial — COM1 debug output.
    serial::init();
    serial_println!("argonOS booting...");

    // Extract bootloader data before taking mutable borrows for display.
    let phys_mem_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("bootloader must provide physical_memory_offset");

    let rsdp_addr = boot_info.rsdp_addr.into_option().unwrap_or(0);

    // SAFETY: We take a raw pointer to memory_regions before passing boot_info
    // to display::init so both can coexist without borrow conflicts.
    let memory_regions: &'static bootloader_api::info::MemoryRegions = {
        let ptr: *const bootloader_api::info::MemoryRegions = &boot_info.memory_regions;
        unsafe { &*ptr }
    };

    // 3. Memory: page table + frame allocator + heap (must come before any alloc).
    memory::init(phys_mem_offset, memory_regions);
    serial_println!("memory: OK  (heap 1 MiB)");

    // 4. Boot splash.
    display::init(boot_info);

    // 5. Keyboard driver.
    drivers::keyboard::init();

    // 6. Storage: AHCI + NVMe (best-effort; log and continue on missing hardware).
    unsafe {
        drivers::ahci::init();
        drivers::nvme::init();
    }

    // 7. SMP: parse MADT, start APs.
    smp::init(rsdp_addr);

    // 8. Filesystem: try FAT32 from disk; fall back to RamFS.
    let has_disk_fs = fs::fat32::init();
    if !has_disk_fs {
        ramfs_demo();
    }

    // 9. Scheduler: spawn tasks and start round-robin.
    future::scheduler::init();
    // Launch the shell as a ring-3 user-mode process.  Falls back to a
    // kernel-mode shell if the ELF can't be loaded (e.g. disk full during build).
    if !launch_shell() {
        future::scheduler::spawn(kernel_shell_task);
    }
    serial_println!("scheduler: running");

    serial_println!("argonOS ready.");

    // Idle loop — the scheduler switches away from here on each timer tick.
    loop {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

// ---------------------------------------------------------------------------
// User-mode shell launch
// ---------------------------------------------------------------------------

/// Load the shell ELF, parse it, create a user address space, and spawn it
/// as a ring-3 process.  Prefers reading from `/bin/shell` on the VFS (M3),
/// then falls back to the compile-time embedded binary.
///
/// Returns `true` on success.
fn launch_shell() -> bool {
    // Try loading from disk first.
    let disk_bytes = load_file_from_vfs("/bin/shell");
    let shell_elf_embedded: &[u8] = include_bytes!(env!("CARGO_BIN_FILE_SHELL_shell"));
    let shell_bytes: &[u8] = if let Some(ref b) = disk_bytes {
        serial_println!("shell: loaded {} bytes from /bin/shell", b.len());
        b.as_slice()
    } else {
        serial_println!("shell: /bin/shell not on disk; using embedded ELF");
        shell_elf_embedded
    };

    let elf = match crate::future::elf::load(shell_bytes) {
        Ok(e) => e,
        Err(e) => {
            serial_println!("shell: ELF load failed: {:?}", e);
            return false;
        }
    };

    let mut addr_space = match crate::memory::address_space::UserAddressSpace::new() {
        Some(a) => a,
        None => {
            serial_println!("shell: OOM creating address space");
            return false;
        }
    };

    let entry = match addr_space.load_elf(&elf) {
        Some(e) => e,
        None => {
            serial_println!("shell: failed to map ELF segments");
            return false;
        }
    };

    // Stack top: one page below USER_STACK_TOP so the first push doesn't fault.
    let user_rsp = crate::memory::address_space::USER_STACK_TOP - 8;
    let cr3 = addr_space.cr3;
    // UserAddressSpace has no Drop impl; frames stay live for the process lifetime.
    let _ = addr_space;

    crate::future::scheduler::spawn_user_process(cr3, entry, user_rsp);
    serial_println!(
        "shell: user-mode process spawned (entry={:#x} rsp={:#x})",
        entry,
        user_rsp
    );
    true
}

// ---------------------------------------------------------------------------
// Kernel-mode interactive shell task
// ---------------------------------------------------------------------------

/// A simple interactive shell running in kernel mode (ring 0).
///
/// This will be replaced by a proper ring-3 user-mode shell once ELF loading
/// and process spawning are fully wired.
fn kernel_shell_task() -> ! {
    serial_println!("shell: kernel-mode shell started");
    loop {
        serial_println!("argonOS> ");
        let mut buf = [0u8; 256];
        let n = drivers::keyboard::readline(&mut buf);
        if n == 0 {
            continue;
        }
        let line = core::str::from_utf8(&buf[..n]).unwrap_or("").trim();
        match line {
            "help" => {
                serial_println!("commands: help echo yield reboot");
            }
            "" => {}
            _ if line.starts_with("echo ") => {
                serial_println!("{}", &line[5..]);
            }
            "yield" => future::scheduler::force_yield(),
            "reboot" => {
                serial_println!("Rebooting...");
                unsafe { x86_64::instructions::port::Port::<u8>::new(0x64).write(0xFE) };
            }
            other => {
                serial_println!("unknown command: {}", other);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read an entire file from the VFS into a heap-allocated Vec.
/// Returns None if the file cannot be opened or read.
fn load_file_from_vfs(path: &str) -> Option<alloc::vec::Vec<u8>> {
    let mut vfs = crate::future::vfs::VFS.lock();
    let fd = vfs.open(path).ok()?;
    let mut data = alloc::vec::Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match vfs.read(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    let _ = vfs.close(fd);
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

// ---------------------------------------------------------------------------
// RamFS demo — mount via VFS
// ---------------------------------------------------------------------------

fn ramfs_demo() {
    // Create and populate RamFS.
    let mut fs = RamFs::new();
    fs.create("/etc/os-release", b"NAME=argonOS\nVERSION=0.1.0-alpha\n")
        .expect("ramfs create");
    fs.create("/boot/motd", b"Welcome to argonOS!\n")
        .expect("ramfs create");

    // Mount at "/" in the global VFS.
    VFS.lock().mount("/", alloc::boxed::Box::new(fs));

    // Round-trip through VFS.
    let fd = VFS.lock().open("/etc/os-release").expect("vfs open");
    let mut buf = [0u8; 64];
    let n = VFS.lock().read(fd, &mut buf).expect("vfs read");
    VFS.lock().close(fd).expect("vfs close");

    let content = core::str::from_utf8(&buf[..n]).unwrap_or("<utf8 error>");
    serial_println!("vfs: /etc/os-release = {:?}", content);

    // List root directory.
    let entries = VFS.lock().readdir("/").expect("vfs readdir");
    serial_println!("vfs: / contains {} entries:", entries.len());
    for e in &entries {
        serial_println!("  {}", e);
    }
    serial_println!("vfs: OK");
}
