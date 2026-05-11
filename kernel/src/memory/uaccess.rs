//! User-space memory access helpers.
//!
//! These functions validate that a pointer from a syscall argument refers to
//! user-space memory (below the canonical user-space limit), then copy data
//! in/out with SMAP bypass (STAC/CLAC) so the kernel can access user pages
//! even though SMAP is enabled.
//!
//! Address space layout:
//!   0x0000_0000_0000_0000 – 0x0000_7FFF_FFFF_FFFF  user space
//!   0xFFFF_8000_0000_0000 – 0xFFFF_FFFF_FFFF_FFFF  kernel space

use core::arch::asm;

/// Highest valid user-space address (inclusive).
pub const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;

/// Check that the range `[addr, addr+len)` lies entirely in user space.
#[inline]
pub fn is_user_range(addr: u64, len: usize) -> bool {
    if len == 0 {
        return addr <= USER_SPACE_END;
    }
    let end = match addr.checked_add(len as u64 - 1) {
        Some(e) => e,
        None => return false,
    };
    end <= USER_SPACE_END
}

/// Copy `len` bytes from user address `src` into kernel slice `dst`.
///
/// Returns `false` if the source range is not fully in user space.
///
/// # Safety
/// The caller must ensure `src` is a valid user pointer for `dst.len()` bytes
/// within the current process's address space.
pub unsafe fn copy_from_user(dst: &mut [u8], src: u64) -> bool {
    if !is_user_range(src, dst.len()) {
        return false;
    }
    // STAC: allow kernel access to user pages (bypass SMAP).
    asm!("stac", options(nostack, preserves_flags));
    core::ptr::copy_nonoverlapping(src as *const u8, dst.as_mut_ptr(), dst.len());
    // CLAC: re-enable SMAP protection.
    asm!("clac", options(nostack, preserves_flags));
    true
}

/// Copy `src` bytes from kernel into user address `dst`.
///
/// Returns `false` if the destination range is not fully in user space.
///
/// # Safety
/// The caller must ensure `dst` is a valid user pointer for `src.len()` bytes
/// within the current process's address space.
pub unsafe fn copy_to_user(dst: u64, src: &[u8]) -> bool {
    if !is_user_range(dst, src.len()) {
        return false;
    }
    asm!("stac", options(nostack, preserves_flags));
    core::ptr::copy_nonoverlapping(src.as_ptr(), dst as *mut u8, src.len());
    asm!("clac", options(nostack, preserves_flags));
    true
}

/// Read a null-terminated C string from user space into `buf`.
///
/// Returns the length (excluding NUL) on success, or `None` on invalid
/// pointer or buffer too small.
///
/// # Safety
/// Same safety requirements as `copy_from_user`.
pub unsafe fn strncpy_from_user(buf: &mut [u8], src: u64) -> Option<usize> {
    if !is_user_range(src, 1) {
        return None;
    }
    asm!("stac", options(nostack, preserves_flags));
    let mut i = 0usize;
    loop {
        if i >= buf.len() {
            asm!("clac", options(nostack, preserves_flags));
            return None; // buffer too small
        }
        let byte = *(src as *const u8).add(i);
        buf[i] = byte;
        if byte == 0 {
            asm!("clac", options(nostack, preserves_flags));
            return Some(i);
        }
        i += 1;
    }
}
