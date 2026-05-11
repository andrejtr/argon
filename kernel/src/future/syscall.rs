use crate::serial_println;

/// Syscall dispatch layer.
///
/// Defines the Linux-compatible syscall numbers argonOS will eventually
/// support and a typed dispatch stub.  Full implementation requires a running
/// scheduler, a VFS mount table, and user-mode address validation.
///
/// The entry point (`syscall_entry`) is registered in the MSR_LSTAR register
/// (SYSCALL/SYSRET mechanism) once user-mode processes are added.
///
/// Linux-compatible 64-bit syscall numbers (subset).
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syscall {
    Read = 0,
    Write = 1,
    Open = 2,
    Close = 3,
    Exit = 60,
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
            60 => Syscall::Exit,
            _ => Syscall::Unknown,
        }
    }
}

/// Kernel-side syscall dispatcher.
///
/// Called from the low-level SYSCALL handler (to be written in assembly).
/// Returns the value to place in RAX as the syscall result.
///
/// # Safety
/// All pointer arguments (`arg1`–`arg3`) originate from user space and MUST
/// be validated against the calling process's memory map before use.
/// This stub does no such validation yet and must not be called with real
/// user pointers until proper validation is implemented.
pub fn dispatch(id: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    match Syscall::from(id) {
        Syscall::Write => sys_write(arg0, arg1, arg2),
        Syscall::Read => sys_read(arg0, arg1, arg2),
        Syscall::Exit => sys_exit(arg0),
        Syscall::Open | Syscall::Close => {
            serial_println!("syscall: {:?} not yet implemented", Syscall::from(id));
            u64::MAX // ENOSYS
        }
        Syscall::Unknown => {
            serial_println!("syscall: unknown id={}", id);
            u64::MAX // ENOSYS
        }
    }
}

// ---------------------------------------------------------------------------
// Stub implementations
// ---------------------------------------------------------------------------

fn sys_write(fd: u64, _buf_ptr: u64, len: u64) -> u64 {
    // Stub: only fd=1 (stdout) supported – writes to serial.
    if fd == 1 {
        // SAFETY: NOT safe yet – requires user-address validation.
        // Placeholder: log to serial for debugging.
        serial_println!("sys_write: fd={} len={} (stub)", fd, len);
        len
    } else {
        u64::MAX // EBADF
    }
}

fn sys_read(fd: u64, _buf_ptr: u64, len: u64) -> u64 {
    serial_println!("sys_read: fd={} len={} (stub)", fd, len);
    0 // EOF
}

fn sys_exit(code: u64) -> u64 {
    serial_println!("sys_exit: code={}", code);
    crate::panic::halt_loop();
}
