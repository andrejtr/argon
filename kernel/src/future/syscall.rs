use crate::serial_println;

/// Syscall dispatch layer.
///
/// Defines the Linux-compatible syscall numbers argonOS supports and a typed
/// dispatch stub.  The entry point is the `syscall_dispatch` function called
/// from the naked SYSCALL entry stub in `arch::x86_64::syscall`.
///
/// Linux x86-64 ABI (subset):
///   RAX = syscall number
///   RDI = arg0  RSI = arg1  RDX = arg2  R10 = arg3
///   Return value in RAX.  On error RAX = -errno (negative).
///
/// argonOS-specific calls occupy the range 400+.
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syscall {
    Read = 0,
    Write = 1,
    Open = 2,
    Close = 3,
    /// yield — voluntarily give up the CPU time-slice.
    SchedYield = 24,
    /// getpid — return the PID of the calling process.
    Getpid = 39,
    Exit = 60,
    /// argonOS: spawn a kernel task by entry-point address.
    Spawn = 400,
    /// Any number not yet implemented.
    Unknown,
}

impl From<u64> for Syscall {
    fn from(n: u64) -> Self {
        match n {
            0 => Syscall::Read,
            1 => Syscall::Write,
            2 => Syscall::Open,
            3 => Syscall::Close,
            24 => Syscall::SchedYield,
            39 => Syscall::Getpid,
            60 => Syscall::Exit,
            400 => Syscall::Spawn,
            _ => Syscall::Unknown,
        }
    }
}

/// Error codes (negated in RAX on error, matching Linux convention).
const ENOSYS: u64 = u64::MAX - 38; // -ENOSYS
const EBADF: u64 = u64::MAX - 9; // -EBADF

/// Kernel-side syscall dispatcher.
///
/// Called from the SYSCALL entry stub.  All pointer arguments MUST be
/// validated against the calling process's memory map before use.
pub fn dispatch(id: u64, arg0: u64, arg1: u64, arg2: u64, _arg3: u64) -> u64 {
    match Syscall::from(id) {
        Syscall::Write => sys_write(arg0, arg1, arg2),
        Syscall::Read => sys_read(arg0, arg1, arg2),
        Syscall::Open => sys_open(arg1),
        Syscall::Close => sys_close(arg0),
        Syscall::SchedYield => sys_yield(),
        Syscall::Getpid => sys_getpid(),
        Syscall::Exit => sys_exit(arg0),
        Syscall::Spawn => sys_spawn(arg0),
        Syscall::Unknown => {
            serial_println!("syscall: unknown id={}", id);
            ENOSYS
        }
    }
}

// ---------------------------------------------------------------------------
// Implementations
// ---------------------------------------------------------------------------

fn sys_write(fd: u64, _buf_ptr: u64, len: u64) -> u64 {
    if fd == 1 || fd == 2 {
        // TODO: when user-mode is live, copy buf from user address space.
        serial_println!("sys_write: fd={} len={} (stub)", fd, len);
        len
    } else {
        EBADF
    }
}

fn sys_read(fd: u64, _buf_ptr: u64, len: u64) -> u64 {
    serial_println!("sys_read: fd={} len={} (stub)", fd, len);
    0 // EOF
}

fn sys_open(path_ptr: u64) -> u64 {
    // TODO: validate user pointer, read null-terminated path.
    serial_println!("sys_open: path_ptr={:#x} (stub)", path_ptr);
    ENOSYS
}

fn sys_close(fd: u64) -> u64 {
    serial_println!("sys_close: fd={}", fd);
    0
}

fn sys_yield() -> u64 {
    // Force an immediate scheduler tick so the current task is preempted.
    crate::future::scheduler::force_yield();
    0
}

fn sys_getpid() -> u64 {
    crate::future::scheduler::current_pid()
        .map(|p| p.0 as u64)
        .unwrap_or(0)
}

fn sys_exit(code: u64) -> u64 {
    serial_println!("sys_exit: code={}", code);
    crate::future::scheduler::exit_current(code as i32);
    0
}

fn sys_spawn(_entry_ptr: u64) -> u64 {
    // Spawning an arbitrary function pointer from userland is not yet safe —
    // requires ELF loading + page-table isolation.  Return ENOSYS for now.
    ENOSYS
}
