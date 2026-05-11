use crate::arch::x86_64::gdt::DOUBLE_FAULT_IST_INDEX;
use crate::arch::x86_64::pic::{IRQ_KEYBOARD, IRQ_TIMER};
/// Interrupt Descriptor Table (IDT) setup.
///
/// Installs handlers for all CPU exceptions argonOS currently cares about.
/// Unhandled exceptions loop-halt via the default `x86_64` stub.
use crate::serial_println;
use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

/// INT 0x80 — Linux-compatible syscall gate (ring-3 callable once user-mode exists).
pub const SYSCALL_VECTOR: u8 = 0x80;

static IDT: Once<InterruptDescriptorTable> = Once::new();

/// Build and load the IDT.  Must be called *after* `gdt::init()`.
pub fn init() {
    let idt = IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();

        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.general_protection_fault.set_handler_fn(gpf_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.stack_segment_fault
            .set_handler_fn(stack_segment_handler);
        idt.invalid_tss.set_handler_fn(invalid_tss_handler);
        idt.segment_not_present.set_handler_fn(snp_handler);

        // Double fault must run on a known-good IST stack so that a
        // kernel-stack-overflow doesn't cause a triple fault.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(DOUBLE_FAULT_IST_INDEX);
        }

        // Hardware IRQs (PIC-remapped to vectors 32+).  IDT is indexed by u8.
        idt[IRQ_TIMER].set_handler_fn(timer_handler);
        idt[IRQ_KEYBOARD].set_handler_fn(keyboard_handler);

        // Syscall gate (INT 0x80, vector 128).
        idt[SYSCALL_VECTOR]
            .set_handler_fn(syscall_handler)
            .set_privilege_level(x86_64::PrivilegeLevel::Ring3);

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
    serial_println!(
        "EXCEPTION: GENERAL PROTECTION FAULT (error=0x{:x})\n{:#?}",
        error,
        frame
    );
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
    serial_println!(
        "EXCEPTION: STACK SEGMENT FAULT (error=0x{:x})\n{:#?}",
        error,
        frame
    );
    crate::panic::halt_loop();
}

extern "x86-interrupt" fn invalid_tss_handler(frame: InterruptStackFrame, error: u64) {
    serial_println!("EXCEPTION: INVALID TSS (error=0x{:x})\n{:#?}", error, frame);
    crate::panic::halt_loop();
}

extern "x86-interrupt" fn snp_handler(frame: InterruptStackFrame, error: u64) {
    serial_println!(
        "EXCEPTION: SEGMENT NOT PRESENT (error=0x{:x})\n{:#?}",
        error,
        frame
    );
    crate::panic::halt_loop();
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, error: u64) -> ! {
    serial_println!(
        "EXCEPTION: DOUBLE FAULT (error=0x{:x})\n{:#?}",
        error,
        frame
    );
    crate::panic::halt_loop();
}

// ---------------------------------------------------------------------------
// Hardware IRQ handlers
// ---------------------------------------------------------------------------

extern "x86-interrupt" fn timer_handler(_frame: InterruptStackFrame) {
    crate::arch::x86_64::pit::tick();
    crate::future::scheduler::on_tick();
    crate::arch::x86_64::pic::end_of_interrupt(crate::arch::x86_64::pic::IRQ_TIMER);
}

extern "x86-interrupt" fn keyboard_handler(_frame: InterruptStackFrame) {
    // Read scancode from data port and route it to the keyboard driver.
    let scancode: u8 = unsafe { x86_64::instructions::port::Port::new(0x60).read() };
    crate::drivers::keyboard::push_scancode(scancode);
    crate::arch::x86_64::pic::end_of_interrupt(crate::arch::x86_64::pic::IRQ_KEYBOARD);
}

// ---------------------------------------------------------------------------
// Syscall gate — INT 0x80
// ---------------------------------------------------------------------------

/// Minimal INT 0x80 handler.
///
/// Convention (Linux-compatible):
///   RAX = syscall number  RDI = arg0  RSI = arg1  RDX = arg2
///   Return value in RAX.
///
/// Until user-mode processes exist this is called only from kernel code in
/// tests / demos.  The `set_privilege_level(Ring3)` on the IDT entry means
/// user-mode will be able to invoke it once ring-3 is set up.
extern "x86-interrupt" fn syscall_handler(_frame: InterruptStackFrame) {
    serial_println!("syscall: INT 0x80 received");
}
