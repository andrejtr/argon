//! Minimal per-CPU data area for single-core bootstrap.
//!
//! On x86-64 the `swapgs` instruction swaps the GS.base MSR with the
//! IA32_KERNEL_GS_BASE MSR.  We use GS to store a tiny per-CPU record so
//! the SYSCALL entry stub can switch stacks atomically.
//!
//! Layout of the `PerCpu` struct (must stay in sync with the `gs:N` offsets
//! used in `syscall.rs`):
//!   Offset 0  (gs:0)  — kernel RSP top (loaded on SYSCALL entry)
//!   Offset 8  (gs:8)  — scratch slot for saving user RSP

use x86_64::registers::model_specific::{GsBase, KernelGsBase};
use x86_64::VirtAddr;

use crate::arch::x86_64::gdt::KERNEL_STACK;

/// Per-CPU record (one instance for the BSP; AP cores get their own when SMP
/// is added).
#[repr(C)]
pub struct PerCpu {
    /// Top of the kernel syscall stack (set once, read on every SYSCALL entry).
    pub kernel_rsp: u64,
    /// Scratch slot — holds the user RSP during SYSCALL so the naked stub can
    /// reference it via `gs:8` before switching stacks.
    pub user_rsp_scratch: u64,
}

/// BSP PerCpu instance (statically allocated; AP instances will be heap-alloc'd).
static mut BSP_PERCPU: PerCpu = PerCpu {
    kernel_rsp: 0,
    user_rsp_scratch: 0,
};

/// Initialise the GS base MSRs for the bootstrap processor.
///
/// After this call:
///  * `KernelGsBase` (swapped in by `swapgs` on SYSCALL entry) points to `BSP_PERCPU`.
///  * `GsBase` points to the same record while we are in kernel mode so that
///    kernel code can access per-CPU data via the GS prefix.
///
/// # Safety
/// Must be called after the GDT/TSS are loaded (so `KERNEL_STACK` has its
/// final address) and before SYSCALL is enabled.
pub unsafe fn init() {
    // Compute the kernel stack top.
    let kstack_top = {
        let base = VirtAddr::from_ptr(&raw const KERNEL_STACK);
        (base + (64usize * 1024) as u64).as_u64()
    };

    BSP_PERCPU.kernel_rsp = kstack_top;
    BSP_PERCPU.user_rsp_scratch = 0;

    let percpu_addr = VirtAddr::from_ptr(&raw const BSP_PERCPU);
    // KernelGsBase is what swapgs swaps INTO GsBase on ring-0 entry.
    KernelGsBase::write(percpu_addr);
    // Set GsBase too so kernel code can access it without swapgs.
    GsBase::write(percpu_addr);

    crate::serial_println!(
        "percpu: BSP PerCpu @ {:#x}  kstack={:#x}",
        percpu_addr.as_u64(),
        kstack_top
    );
}

/// Update the kernel RSP stored in the BSP PerCpu area.
///
/// Called by the scheduler before each ring-3 switch so SYSCALL/interrupt
/// entry always loads the new task's kernel stack.
pub fn set_kernel_rsp(rsp: u64) {
    // SAFETY: single-core BSP; scheduler lock guarantees exclusive access.
    unsafe { BSP_PERCPU.kernel_rsp = rsp };
}
