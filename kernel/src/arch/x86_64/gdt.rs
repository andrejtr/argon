/// Global Descriptor Table (GDT) + Task State Segment (TSS).
///
/// We set up the minimal GDT required to enter 64-bit protected mode:
///   0. Null descriptor (required by the CPU)
///   1. Kernel code segment (DPL 0, 64-bit)
///   2. Kernel data segment (DPL 0)
///   3. TSS descriptor (needed for interrupt stack switching)
///
/// The TSS defines the Interrupt Stack Table (IST) so that critical exceptions
/// such as Double Fault (#DF) always use a known-good stack.
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// IST index for the double-fault handler stack.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// Size of the emergency stack for critical exception handlers (8 KiB).
const STACK_SIZE: usize = 8 * 1024;

// Static storage for the TSS emergency stack.
static mut DOUBLE_FAULT_STACK: [u8; STACK_SIZE] = [0u8; STACK_SIZE];

struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

use spin::Once;
static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();
static TSS: Once<TaskStateSegment> = Once::new();

/// Initialise and load the GDT.
///
/// Must be called before setting up the IDT and before any interrupt can fire.
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS};
    use x86_64::instructions::tables::load_tss;

    let tss = TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            let stack_start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
            stack_start + STACK_SIZE as u64
        };
        tss
    });

    let (gdt, selectors) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        gdt.append(Descriptor::kernel_data_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(tss));
        (
            gdt,
            Selectors {
                code_selector,
                tss_selector,
            },
        )
    });

    gdt.load();

    // SAFETY: The GDT we just loaded contains valid selectors.
    unsafe {
        CS::set_reg(selectors.code_selector);
        load_tss(selectors.tss_selector);
    }
}
