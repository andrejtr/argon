//! Ring-3 (user-mode) entry via `iretq`.
//!
//! `enter_user_mode` is the one-way door from kernel mode into a user process.
//! It loads the process's CR3, updates the kernel stack pointer in the TSS
//! and PerCpu area, then executes `iretq` with a carefully crafted stack
//! frame that transitions the CPU to DPL-3.
//!
//! `iretq` frame layout (pushed before `iretq`, top-of-stack first):
//!   [RSP+0]  RIP    — user entry point
//!   [RSP+8]  CS     — user code selector (DPL 3)
//!   [RSP+16] RFLAGS — IF=1, IOPL=0, reserved bit 1 set
//!   [RSP+24] RSP    — user stack top
//!   [RSP+32] SS     — user data selector (DPL 3)

use core::arch::asm;

use x86_64::{
    registers::control::{Cr3, Cr3Flags},
    structures::paging::PhysFrame,
};

use crate::arch::x86_64::gdt::selectors;

/// Saved kernel RSP for SYSCALL reentry — updated before each ring-3 entry.
pub static mut CURRENT_KERNEL_RSP: u64 = 0;

/// Switch to `cr3`, set up an `iretq` frame, and jump to `entry` in ring 3.
///
/// Never returns.
///
/// # Safety
/// * `cr3` must be a valid L4 page table frame that maps both the user
///   entry code and the kernel (upper half).
/// * `user_rsp` must point to a mapped, writable user-space stack.
/// * Called with interrupts disabled is recommended but not required; `iretq`
///   restores RFLAGS (including IF) from the frame.
pub unsafe fn enter_user_mode(entry: u64, user_rsp: u64, cr3: PhysFrame) -> ! {
    // Switch page table to the user process's L4.
    Cr3::write(cr3, Cr3Flags::empty());

    let sel = selectors();
    // RPL 3 selectors: the CPU checks that CS/SS DPL == RPL on iretq.
    let user_cs = (sel.user_code.0 | 3) as u64;
    let user_ss = (sel.user_data.0 | 3) as u64;
    // RFLAGS: bit 1 (reserved, always 1), IF (bit 9) to enable interrupts.
    let rflags: u64 = 0x202;

    asm!(
        // Clear most GPRs so user code starts with a clean slate.
        "xor rbx, rbx",
        "xor rcx, rcx",
        "xor rdx, rdx",
        "xor rsi, rsi",
        "xor rdi, rdi",
        "xor rbp, rbp",
        "xor r8,  r8",
        "xor r9,  r9",
        "xor r10, r10",
        "xor r11, r11",
        "xor r12, r12",
        "xor r13, r13",
        "xor r14, r14",
        "xor r15, r15",
        // Build iretq frame on the current (kernel) stack.
        "push {ss}",       // SS (user)
        "push {user_rsp}", // RSP (user)
        "push {rflags}",   // RFLAGS
        "push {cs}",       // CS (user)
        "push {rip}",      // RIP (entry)
        "iretq",
        ss       = in(reg) user_ss,
        user_rsp = in(reg) user_rsp,
        rflags   = in(reg) rflags,
        cs       = in(reg) user_cs,
        rip      = in(reg) entry,
        options(noreturn),
    );
}
