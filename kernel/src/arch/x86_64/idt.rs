/// Interrupt Descriptor Table (IDT) setup.
///
/// Installs handlers for all CPU exceptions argonOS currently cares about.
/// Unhandled exceptions loop-halt via the default `x86_64` stub.
use crate::serial_println;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use spin::Once;
use crate::arch::x86_64::gdt::DOUBLE_FAULT_IST_INDEX;

static IDT: Once<InterruptDescriptorTable> = Once::new();

/// Build and load the IDT.  Must be called *after* `gdt::init()`.
pub fn init() {
    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.general_protection_fault.set_handler_fn(gpf_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.stack_segment_fault.set_handler_fn(stack_segment_handler);
        idt.invalid_tss.set_handler_fn(invalid_tss_handler);
        idt.segment_not_present.set_handler_fn(snp_handler);

        // Double fault must run on a known-good IST stack so that a
        // kernel-stack-overflow doesn't cause a triple fault.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);
        }

        idt
    });

    idt.load();
}

// ---------------------------------------------------------------------------
// Exception handlers
// ---------------------------------------------------------------------------

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    serial_println!("EXCEPTION: BREAKPOINT\n{:#?}", frame);
}

extern "x86-interrupt" fn gpf_handler(frame: InterruptStackFrame, error: u64) {
    serial_println!("EXCEPTION: GENERAL PROTECTION FAULT (error=0x{:x})\n{:#?}", error, frame);
    crate::panic::halt_loop();
}

extern "x86-interrupt" fn page_fault_handler(
    frame: InterruptStackFrame,
    error: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    serial_println!(
        "EXCEPTION: PAGE FAULT\n  addr={:#x}  error={:?}\n{:#?}",
        Cr2::read_raw(),
        error,
        frame
    );
    crate::panic::halt_loop();
}

extern "x86-interrupt" fn stack_segment_handler(frame: InterruptStackFrame, error: u64) {
    serial_println!("EXCEPTION: STACK SEGMENT FAULT (error=0x{:x})\n{:#?}", error, frame);
    crate::panic::halt_loop();
}

extern "x86-interrupt" fn invalid_tss_handler(frame: InterruptStackFrame, error: u64) {
    serial_println!("EXCEPTION: INVALID TSS (error=0x{:x})\n{:#?}", error, frame);
    crate::panic::halt_loop();
}

extern "x86-interrupt" fn snp_handler(frame: InterruptStackFrame, error: u64) {
    serial_println!("EXCEPTION: SEGMENT NOT PRESENT (error=0x{:x})\n{:#?}", error, frame);
    crate::panic::halt_loop();
}

extern "x86-interrupt" fn double_fault_handler(
    frame: InterruptStackFrame,
    error: u64,
) -> ! {
    serial_println!("EXCEPTION: DOUBLE FAULT (error=0x{:x})\n{:#?}", error, frame);
    crate::panic::halt_loop();
}
