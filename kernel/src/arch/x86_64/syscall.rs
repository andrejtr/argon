//! SYSCALL / SYSRET fast system-call entry point.
//!
//! On x86-64 the preferred mechanism is the `SYSCALL` instruction (not INT 0x80).
//! The CPU transfers control to the address in IA32_LSTAR, with:
//!   * CS  = IA32_STAR[47:32]       (kernel code selector)
//!   * SS  = IA32_STAR[47:32] + 8   (kernel data selector)
//!   * RCX = return RIP (saved automatically by SYSCALL)
//!   * R11 = saved RFLAGS
//!   * RFLAGS are masked with ~IA32_FMASK
//!
//! Convention (Linux ABI — our userland will follow the same):
//!   RAX = syscall number
//!   RDI, RSI, RDX, R10, R8, R9 = arguments (R10 instead of RCX!)
//!   RAX = return value

use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};
use x86_64::registers::rflags::RFlags;

use crate::arch::x86_64::gdt::selectors;
use crate::serial_println;

/// Enable the SYSCALL/SYSRET instruction pair and install the entry stub.
///
/// # Safety
/// Must be called after `gdt::init()`.  Writes to model-specific registers.
pub unsafe fn init() {
    // Enable the SCE (System Call Enable) bit in EFER.
    let mut efer = Efer::read();
    efer.insert(EferFlags::SYSTEM_CALL_EXTENSIONS);
    Efer::write(efer);

    // STAR: bits [47:32] = kernel CS selector, bits [63:48] = user CS − 16 base.
    // The x86_64 crate's Star::write takes (ring3_cs, ring3_ds, ring0_cs).
    let sel = selectors();
    Star::write(
        sel.user_code,
        sel.user_data,
        sel.kernel_code,
        sel.kernel_data,
    )
    .expect("invalid STAR selectors");

    // LSTAR: address of the raw ASM entry stub.
    LStar::write(x86_64::VirtAddr::new(
        syscall_entry as *const () as usize as u64,
    ));

    // FMASK: mask RFLAGS bits we want cleared on SYSCALL entry.
    // Clear IF (disable interrupts during syscall dispatch) + DF (direction flag).
    SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::DIRECTION_FLAG);

    serial_println!("syscall: SYSCALL/SYSRET MSRs configured");
}

/// Raw SYSCALL entry point.
///
/// The CPU arrives here with:
///   - Interrupts disabled (we masked IF in FMASK)
///   - RCX = user RIP to return to   R11 = user RFLAGS
///   - RSP still pointing at the *user* stack (we must switch immediately)
///   - Everything else as the user left it
///
/// We switch to the per-CPU kernel stack (stored in TSS.RSP0), save the
/// minimal clobber set, call the Rust dispatcher, restore and SYSRET.
///
/// # Safety
/// Called directly by the CPU; the `#[naked]` attribute means we control the
/// entire function body via `asm!`.
#[unsafe(naked)]
unsafe extern "C" fn syscall_entry() {
    // Naked function — no Rust prologue/epilogue.
    // Stack layout after we push everything (grows downward):
    //   [rsp+ 0] rax (syscall number / return value)
    //   [rsp+ 8] rdi (arg 0)
    //   [rsp+16] rsi (arg 1)
    //   [rsp+24] rdx (arg 2)
    //   [rsp+32] r10 (arg 3)
    //   [rsp+40] r8  (arg 4)
    //   [rsp+48] r9  (arg 5)
    //   [rsp+56] rcx (user RIP)
    //   [rsp+64] r11 (user RFLAGS)
    //   [rsp+72] user RSP (saved before swap)
    core::arch::naked_asm!(
        // --- swap to kernel stack via TSS.RSP0 ---
        // Save user RSP, load kernel RSP from TSS using swapgs / GS base trick
        // For now we use a simple per-kernel global stack (single-CPU safe).
        "swapgs",                        // swap GS base (kernel GS base = per-cpu data)
        "mov    gs:8, rsp",              // save user rsp in per-cpu scratch
        "mov    rsp, gs:0",              // load kernel rsp from per-cpu area
        // --- push all caller-saved + syscall registers ---
        "push   gs:8",                   // user rsp
        "push   r11",                    // user rflags
        "push   rcx",                    // user rip
        "push   r9",
        "push   r8",
        "push   r10",
        "push   rdx",
        "push   rsi",
        "push   rdi",
        "push   rax",
        // --- call Rust dispatcher ---
        // fn syscall_dispatch(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64
        "mov    rdi, rax",              // syscall number
        "mov    rsi, [rsp+8]",          // arg0 (rdi saved above)
        "mov    rdx, [rsp+16]",         // arg1 (rsi)
        "mov    rcx, [rsp+24]",         // arg2 (rdx)
        "mov    r8,  [rsp+32]",         // arg3 (r10)
        "sti",                           // re-enable interrupts during dispatch
        "call   {dispatch}",
        "cli",                           // disable before SYSRET
        // RAX now holds return value.
        "pop    rax",                    // discard saved rax, keep dispatcher return in rax
        "add    rsp, 8",                 // skip rdi
        "pop    rdi",                    // nope — we actually want rdi for restore:
        // Restore in reverse:
        "pop    rdi",
        "pop    rsi",
        "pop    rdx",
        "pop    r10",
        "pop    r8",
        "pop    r9",
        "pop    rcx",                    // user rip
        "pop    r11",                    // user rflags
        "pop    rsp",                    // restore user rsp  (last pop!)
        "swapgs",
        "sysretq",
        dispatch = sym syscall_dispatch,
    );
}

/// Rust-level syscall dispatcher — called from the naked asm stub.
///
/// Arguments match the Linux x86-64 ABI (rdi=nr, rsi=a0 … r8=a4).
#[unsafe(no_mangle)]
extern "C" fn syscall_dispatch(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    crate::future::syscall::dispatch(nr, a0, a1, a2, a3)
}
