//! argon-user — minimal userspace runtime library for argonOS.
//!
//! Targets `x86_64-unknown-none` (no OS, no std).  Provides:
//!   * Syscall wrappers using the Linux x86-64 ABI.
//!   * A minimal `println!` macro that writes via `sys_write(fd=1)`.
//!   * A `panic_handler` that calls `sys_exit(1)`.
//!
//! argonOS syscall ABI (matches Linux x86-64):
//!   SYSCALL; RAX = nr; RDI/RSI/RDX/R10/R8/R9 = args; RAX = return.
#![no_std]

// ---------------------------------------------------------------------------
// Syscall numbers (matches kernel/src/future/syscall.rs)
// ---------------------------------------------------------------------------
pub mod nr {
    pub const READ: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const OPEN: u64 = 2;
    pub const CLOSE: u64 = 3;
    pub const SCHED_YIELD: u64 = 24;
    pub const GETPID: u64 = 39;
    pub const EXIT: u64 = 60;
    pub const REBOOT: u64 = 169;
    pub const SPAWN: u64 = 400;
    pub const READDIR: u64 = 401;
}

// ---------------------------------------------------------------------------
// Raw syscall wrappers
// ---------------------------------------------------------------------------

/// Issue a syscall with 0 arguments.
///
/// # Safety
/// The caller must ensure `nr` is a valid argonOS syscall number and that
/// calling it is sound in the current execution context.
#[inline(always)]
pub unsafe fn syscall0(nr: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") nr => ret,
        out("rcx") _,
        out("r11") _,
        options(nostack),
    );
    ret
}

/// Issue a syscall with 1 argument.
///
/// # Safety
/// The caller must ensure `nr` is a valid argonOS syscall number, that `a0`
/// satisfies any requirements of that syscall, and that calling it is sound.
#[inline(always)]
pub unsafe fn syscall1(nr: u64, a0: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") nr => ret,
        in("rdi") a0,
        out("rcx") _,
        out("r11") _,
        options(nostack),
    );
    ret
}

/// Issue a syscall with 3 arguments.
///
/// # Safety
/// The caller must ensure `nr` is a valid argonOS syscall number and that
/// `a0`–`a2` satisfy the argument requirements of that syscall.
#[inline(always)]
pub unsafe fn syscall3(nr: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") nr => ret,
        in("rdi") a0,
        in("rsi") a1,
        in("rdx") a2,
        out("rcx") _,
        out("r11") _,
        options(nostack),
    );
    ret
}

// ---------------------------------------------------------------------------
// High-level wrappers
// ---------------------------------------------------------------------------

pub fn write(fd: u64, buf: &[u8]) -> isize {
    unsafe { syscall3(nr::WRITE, fd, buf.as_ptr() as u64, buf.len() as u64) as isize }
}

pub fn read(fd: u64, buf: &mut [u8]) -> isize {
    unsafe { syscall3(nr::READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64) as isize }
}

pub fn exit(code: i32) -> ! {
    unsafe { syscall1(nr::EXIT, code as u64) };
    // Should never reach here — spin in case the kernel doesn't terminate us.
    loop {
        core::hint::spin_loop();
    }
}

pub fn getpid() -> u32 {
    unsafe { syscall0(nr::GETPID) as u32 }
}

pub fn sched_yield() {
    unsafe { syscall0(nr::SCHED_YIELD) };
}

/// Open a file at `path`.  Returns the kernel fd on success (>= 0), or a
/// negative errno on failure.
pub fn open(path: &str) -> i64 {
    unsafe { syscall1(nr::OPEN, path.as_ptr() as u64) as i64 }
}

/// Close a kernel file descriptor.
pub fn close(fd: u64) -> i64 {
    unsafe { syscall1(nr::CLOSE, fd) as i64 }
}

/// List directory entries at `path` into `buf` as newline-separated names.
/// Returns the number of bytes written, or a negative errno on failure.
pub fn readdir(path: &str, buf: &mut [u8]) -> isize {
    unsafe {
        syscall3(
            nr::READDIR,
            path.as_ptr() as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        ) as isize
    }
}

/// Reboot the system.  Does not return.
pub fn reboot() -> ! {
    unsafe { syscall0(nr::REBOOT) };
    loop {
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------------
// Minimal I/O
// ---------------------------------------------------------------------------

/// Write a byte slice to stdout (fd 1).
pub fn print(s: &str) {
    write(1, s.as_bytes());
}

/// Write a byte slice + newline to stdout.
pub fn println(s: &str) {
    write(1, s.as_bytes());
    write(1, b"\n");
}

// ---------------------------------------------------------------------------
// println! macro
// ---------------------------------------------------------------------------

/// Minimal `println!` — formats into a fixed 256-byte stack buffer.
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        struct Buf {
            data: [u8; 256],
            pos: usize,
        }
        impl core::fmt::Write for Buf {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let bytes = s.as_bytes();
                let avail = self.data.len() - self.pos;
                let n = bytes.len().min(avail);
                self.data[self.pos..self.pos + n].copy_from_slice(&bytes[..n]);
                self.pos += n;
                Ok(())
            }
        }
        let mut buf = Buf { data: [0u8; 256], pos: 0 };
        let _ = core::write!(buf, $($arg)*);
        $crate::print(unsafe { core::str::from_utf8_unchecked(&buf.data[..buf.pos]) });
        $crate::write(1, b"\n");
    }};
}

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // Best effort: write "panic" then exit.
    write(2, b"argon-user: panic\n");
    exit(1);
}

// ---------------------------------------------------------------------------
// Stack-smashing protector symbols (required by -Z stack-protector=strong)
// ---------------------------------------------------------------------------

#[no_mangle]
pub static __stack_chk_guard: u64 = 0xDEAD_BEEF_DEAD_BEEF;

#[no_mangle]
pub extern "C" fn __stack_chk_fail() -> ! {
    write(2, b"argon-user: stack smashing detected\n");
    exit(1);
}
