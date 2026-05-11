use crate::serial_println;

extern crate alloc;

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

fn sys_write(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if fd == 1 || fd == 2 {
        // Validate that the buffer lives in user address space.
        if !crate::memory::uaccess::is_user_range(buf_ptr, len as usize) {
            return u64::MAX - 13; // -EFAULT
        }
        let mut tmp = alloc::vec![0u8; len as usize];
        // SAFETY: is_user_range validated the address; SMAP is bypassed via STAC/CLAC.
        if !unsafe { crate::memory::uaccess::copy_from_user(tmp.as_mut_slice(), buf_ptr) } {
            return u64::MAX - 13; // -EFAULT
        }
        if let Ok(s) = core::str::from_utf8(&tmp) {
            serial_println!("sys_write({}): {}", fd, s.trim_end());
        }
        len
    } else {
        EBADF
    }
}

fn sys_read(fd: u64, buf_ptr: u64, len: u64) -> u64 {
    if fd != 0 {
        return EBADF;
    }
    if !crate::memory::uaccess::is_user_range(buf_ptr, len as usize) {
        return u64::MAX - 13; // -EFAULT
    }
    // Read one character from the keyboard driver.
    let c = crate::drivers::keyboard::read_char();
    let buf = [c];
    // SAFETY: is_user_range validated the address; STAC/CLAC bypass SMAP.
    if !unsafe { crate::memory::uaccess::copy_to_user(buf_ptr, &buf) } {
        return u64::MAX - 13;
    }
    1
}

fn sys_open(path_ptr: u64) -> u64 {
    // Validate and copy the null-terminated path from user space.
    let mut path_buf = [0u8; 256];
    // SAFETY: strncpy_from_user validates the address range before accessing.
    let len = match unsafe { crate::memory::uaccess::strncpy_from_user(&mut path_buf, path_ptr) } {
        Some(n) => n,
        None => return u64::MAX - 13, // -EFAULT
    };
    let path = match core::str::from_utf8(&path_buf[..len]) {
        Ok(s) => s,
        Err(_) => return u64::MAX - 22, // -EINVAL
    };
    serial_println!("sys_open: path=\"{}\"", path);
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
