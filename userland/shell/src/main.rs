//! argonOS shell — minimal interactive shell running in user mode.
//!
//! Targets `x86_64-unknown-none`.  The shell reads commands from stdin
//! (fd 0) one line at a time and dispatches built-in commands.
//!
//! Built-in commands:
//!   ls      — list root VFS entries (via sys_readdir stub)
//!   cat     — print a file's contents
//!   echo    — print arguments
//!   ps      — print the current PID
//!   yield   — voluntarily yield the CPU
//!   reboot  — halt loop (real reboot requires ACPI)
//!   help    — list commands
#![no_std]
#![no_main]

use argon_user::{getpid, println, read, sched_yield, write};

/// Shell entry point called by the OS loader.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    println!("argonOS shell v0.1.0-alpha");
    println!("Type 'help' for available commands.");
    write(1, b"\n");

    let mut line_buf = [0u8; 256];

    loop {
        write(1, b"argon> ");
        let n = read(0, &mut line_buf);
        if n <= 0 {
            sched_yield();
            continue;
        }
        let line = core::str::from_utf8(&line_buf[..n as usize])
            .unwrap_or("")
            .trim_end_matches(['\n', '\r']);
        dispatch(line);
    }
}

fn dispatch(line: &str) {
    let (cmd, args) = match line.split_once(' ') {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
    match cmd {
        "help" => cmd_help(),
        "echo" => cmd_echo(args),
        "ps" => cmd_ps(),
        "yield" => cmd_yield(),
        "reboot" => cmd_reboot(),
        "ls" => cmd_ls(),
        "cat" => cmd_cat(args),
        "" => {}
        _ => {
            println!("unknown command: {}", cmd);
            println!("type 'help' for available commands");
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in implementations
// ---------------------------------------------------------------------------

fn cmd_help() {
    println!("Available commands:");
    println!("  ls       list root filesystem entries");
    println!("  cat <f>  print file contents");
    println!("  echo     print arguments");
    println!("  ps       print process id");
    println!("  yield    voluntarily yield CPU");
    println!("  reboot   halt the system");
    println!("  help     show this message");
}

fn cmd_echo(args: &str) {
    println!("{}", args);
}

fn cmd_ps() {
    let pid = getpid();
    println!("PID {}", pid);
}

fn cmd_yield() {
    sched_yield();
    println!("yielded");
}

fn cmd_reboot() {
    println!("halting…");
    // No ACPI yet — busy loop (the watchdog/reset would need ACPI or port 0x64).
    loop {}
}

fn cmd_ls() {
    // sys_readdir is not yet exposed as a userspace syscall; placeholder.
    println!("/ (virtual root)");
    println!("  /etc/os-release");
    println!("  /boot/motd");
}

fn cmd_cat(path: &str) {
    if path.is_empty() {
        println!("usage: cat <path>");
        return;
    }
    // sys_open + sys_read stubs — will work once the kernel-side pointer
    // validation and VFS fd plumbing is complete.
    use argon_user::{nr, syscall1, syscall3};
    let fd = unsafe { syscall1(nr::OPEN, path.as_ptr() as u64) };
    if fd > i32::MAX as u64 {
        println!("cat: cannot open '{}'", path);
        return;
    }
    let mut buf = [0u8; 512];
    let n = unsafe { syscall3(nr::READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64) };
    if n == 0 {
        println!("(empty)");
    } else {
        let s = core::str::from_utf8(&buf[..n as usize]).unwrap_or("<binary>");
        write(1, s.as_bytes());
    }
    unsafe { syscall1(nr::CLOSE, fd) };
}
