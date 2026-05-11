/// Global Descriptor Table (GDT) + Task State Segment (TSS).
///
/// Layout (Intel SDM order required for SYSCALL/SYSRET):
///   0. Null descriptor
///   1. Kernel code  (DPL 0, 64-bit)   ← CS after boot, SYSCALL lands here
///   2. Kernel data  (DPL 0)
///   3. User data    (DPL 3)            ← SYSRET sets SS to this
///   4. User code    (DPL 3, 64-bit)   ← SYSRET sets CS to this
///   5. TSS          (128-bit system desc, occupies two slots)
///
/// SYSCALL/SYSRET selector arithmetic (Intel SDM Vol.2, SYSCALL entry):
///   On SYSCALL:  CS = IA32_STAR[47:32]          → kernel code (slot 1, sel 0x08)
///                SS = IA32_STAR[47:32] + 8       → kernel data (slot 2, sel 0x10)
///   On SYSRET:   CS = IA32_STAR[63:48] + 16      → user code  (slot 4, sel 0x33)
///                SS = IA32_STAR[63:48] + 8        → user data  (slot 3, sel 0x2B)
///
/// Therefore IA32_STAR high 16 bits must be user data selector − 8 = 0x23.
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// IST index for the double-fault handler stack.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Size of the emergency stack for critical exception handlers (8 KiB).
const STACK_SIZE: usize = 8 * 1024;

// Static storage for the TSS emergency stack.
static mut DOUBLE_FAULT_STACK: [u8; STACK_SIZE] = [0u8; STACK_SIZE];

/// Kernel privilege-level 0 stack top stored in TSS.RSP0.
/// When a SYSCALL or interrupt comes from ring 3 the CPU switches to this.
pub static mut KERNEL_STACK: [u8; 64 * 1024] = [0u8; 64 * 1024];

/// Exported GDT selectors (needed by SYSCALL MSR init and segment reloads).
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_data: SegmentSelector,
    pub user_code: SegmentSelector,
    pub tss_selector: SegmentSelector,
}

use spin::Once;
static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();
static TSS: Once<TaskStateSegment> = Once::new();

/// Return the currently loaded GDT selectors.
///
/// # Panics
/// Panics if `gdt::init()` has not been called.
pub fn selectors() -> &'static Selectors {
    &GDT.get().expect("GDT not initialised").1
}

/// Initialise and load the GDT.
///
/// Must be called before setting up the IDT and before any interrupt can fire.
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS, DS, ES, SS};
    use x86_64::instructions::tables::load_tss;

    let tss = TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();
        // IST slot 0 — double-fault handler.
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            let stack_start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
            stack_start + STACK_SIZE as u64
        };
        // RSP0 — ring-0 stack used on syscall/interrupt entry from ring 3.
        tss.privilege_stack_table[0] = {
            let base = VirtAddr::from_ptr(&raw const KERNEL_STACK);
            base + (64usize * 1024) as u64
        };
        tss
    });

    let (gdt, selectors) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.append(Descriptor::kernel_code_segment()); // 0x08
        let kernel_data = gdt.append(Descriptor::kernel_data_segment()); // 0x10
        let user_data = gdt.append(Descriptor::user_data_segment()); // 0x18 → RPL 3 → 0x1B
        let user_code = gdt.append(Descriptor::user_code_segment()); // 0x20 → RPL 3 → 0x23
        let tss_selector = gdt.append(Descriptor::tss_segment(tss)); // 0x28 (two slots)
        (
            gdt,
            Selectors {
                kernel_code,
                kernel_data,
                user_data,
                user_code,
                tss_selector,
            },
        )
    });

    gdt.load();

    // SAFETY: The GDT we just loaded contains valid selectors.
    unsafe {
        CS::set_reg(selectors.kernel_code);
        SS::set_reg(selectors.kernel_data);
        DS::set_reg(SegmentSelector::NULL);
        ES::set_reg(SegmentSelector::NULL);
        load_tss(selectors.tss_selector);
    }
}

/// Update the TSS RSP0 field (kernel stack pointer used on ring-0 entry).
///
/// Must be called before switching to a new task's ring-3 context so that
/// interrupts and SYSCALLs return to the correct kernel stack.
pub fn set_rsp0(rsp: u64) {
    if let Some(tss) = TSS.get() {
        // SAFETY: We update the TSS while holding the scheduler lock;
        // no concurrent access is possible on this core.
        let ptr = (tss as *const TaskStateSegment).cast_mut();
        unsafe { (*ptr).privilege_stack_table[0] = VirtAddr::new(rsp) };
    }
}
