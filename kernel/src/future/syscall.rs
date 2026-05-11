use crate::future::vfs::VFS;
use crate::serial_println;

extern crate alloc;

use crate::future::vfs::Fd as VfsFd;
/// Global kernel file-descriptor table.
///
/// stdin(0)/stdout(1)/stderr(2) are always terminals; fds ≥3 are VFS files.
/// Maps kernel_fd → VFS Fd.
use alloc::collections::BTreeMap;
use core::sync::atomic::AtomicU32;
use spin::Mutex;

static FDTABLE: Mutex<BTreeMap<u32, VfsFd>> = Mutex::new(BTreeMap::new());
static NEXT_FD: AtomicU32 = AtomicU32::new(3);

fn alloc_kfd(vfs_fd: VfsFd) -> u32 {
    let kfd = NEXT_FD.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    FDTABLE.lock().insert(kfd, vfs_fd);
    kfd
}

fn lookup_kfd(kfd: u32) -> Option<VfsFd> {
    FDTABLE.lock().get(&kfd).copied()
}

fn free_kfd(kfd: u32) -> Option<VfsFd> {
    FDTABLE.lock().remove(&kfd)
}

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
    /// reboot — reset the machine via the PS/2 keyboard controller.
    Reboot = 169,
    /// argonOS: spawn a kernel task by entry-point address.
    Spawn = 400,
    /// argonOS: list directory entries.
    Readdir = 401,
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
            169 => Syscall::Reboot,
            400 => Syscall::Spawn,
            401 => Syscall::Readdir,
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
        Syscall::Open => sys_open(arg0), // arg0 = rdi = path pointer
        Syscall::Close => sys_close(arg0),
        Syscall::SchedYield => sys_yield(),
        Syscall::Getpid => sys_getpid(),
        Syscall::Exit => sys_exit(arg0),
        Syscall::Reboot => sys_reboot(),
        Syscall::Spawn => sys_spawn(arg0),
        Syscall::Readdir => sys_readdir(arg0, arg1, arg2),
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
    if !crate::memory::uaccess::is_user_range(buf_ptr, len as usize) {
        return u64::MAX - 13; // -EFAULT
    }
    if fd == 0 {
        // Read a full line from the keyboard (blocks until Enter is pressed).
        let mut tmp = alloc::vec![0u8; len as usize];
        let n = crate::drivers::keyboard::readline(&mut tmp);
        if !unsafe { crate::memory::uaccess::copy_to_user(buf_ptr, &tmp[..n]) } {
            return u64::MAX - 13;
        }
        n as u64
    } else {
        // Read from an open VFS file descriptor.
        let vfs_fd = match lookup_kfd(fd as u32) {
            Some(f) => f,
            None => return EBADF,
        };
        let mut tmp = alloc::vec![0u8; len as usize];
        let n = match VFS.lock().read(vfs_fd, &mut tmp) {
            Ok(n) => n,
            Err(_) => return EBADF,
        };
        if !unsafe { crate::memory::uaccess::copy_to_user(buf_ptr, &tmp[..n]) } {
            return u64::MAX - 13;
        }
        n as u64
    }
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
    match VFS.lock().open(path) {
        Ok(vfs_fd) => {
            let kfd = alloc_kfd(vfs_fd);
            serial_println!("sys_open: \"{}\" → kfd={}", path, kfd);
            kfd as u64
        }
        Err(_) => {
            serial_println!("sys_open: not found \"{}\"", path);
            u64::MAX - 2 // -ENOENT
        }
    }
}

fn sys_close(fd: u64) -> u64 {
    if let Some(vfs_fd) = free_kfd(fd as u32) {
        let _ = VFS.lock().close(vfs_fd);
        serial_println!("sys_close: kfd={}", fd);
    }
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

/// List directory entries at `path` into the user buffer as newline-separated
/// entry names.  Returns the number of bytes written on success.
fn sys_readdir(path_ptr: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    let mut path_buf = [0u8; 256];
    let len = match unsafe { crate::memory::uaccess::strncpy_from_user(&mut path_buf, path_ptr) } {
        Some(n) => n,
        None => return u64::MAX - 13, // -EFAULT
    };
    let path = match core::str::from_utf8(&path_buf[..len]) {
        Ok(s) => s,
        Err(_) => return u64::MAX - 22, // -EINVAL
    };
    let entries = match VFS.lock().readdir(path) {
        Ok(e) => e,
        Err(_) => return u64::MAX - 2, // -ENOENT
    };
    // Encode entries as a newline-separated list.
    let mut out = alloc::string::String::new();
    for e in &entries {
        out.push_str(e.as_str());
        out.push('\n');
    }
    let out_bytes = out.as_bytes();
    if !crate::memory::uaccess::is_user_range(buf_ptr, buf_len as usize) {
        return u64::MAX - 13;
    }
    let copy_len = out_bytes.len().min(buf_len as usize);
    if !unsafe { crate::memory::uaccess::copy_to_user(buf_ptr, &out_bytes[..copy_len]) } {
        return u64::MAX - 13;
    }
    copy_len as u64
}

/// Reboot the system via the PS/2 keyboard controller reset line.
fn sys_reboot() -> u64 {
    serial_println!("sys_reboot: rebooting via port 0x64");
    unsafe { x86_64::instructions::port::Port::<u8>::new(0x64).write(0xFE) };
    // Spin until the reset takes effect.
    loop {
        x86_64::instructions::hlt();
    }
}
