/// 8259 Programmable Interrupt Controller (PIC) driver.
///
/// The x86 PC has two cascaded 8259 PICs.  Their default IRQ vectors (0–15)
/// overlap with CPU exception vectors (0–31), causing spurious interrupts.
/// We remap them to vectors 32–47 before enabling hardware interrupts.
use x86_64::instructions::port::Port;

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// First IRQ vector after remapping (vectors 32–39 = IRQs 0–7).
pub const PIC1_OFFSET: u8 = 32;
/// Second PIC offset (vectors 40–47 = IRQs 8–15).
pub const PIC2_OFFSET: u8 = PIC1_OFFSET + 8;

/// IRQ0 (timer) vector.
pub const IRQ_TIMER: u8 = PIC1_OFFSET;
/// IRQ1 (keyboard) vector.
pub const IRQ_KEYBOARD: u8 = PIC1_OFFSET + 1;

const ICW1_INIT: u8 = 0x11; // initialise + ICW4 needed
const ICW4_8086: u8 = 0x01; // 8086/88 mode

/// Remap both PICs and mask all IRQs except the timer (IRQ0).
///
/// # Safety
/// Must be called exactly once, before `sti`.
pub unsafe fn init() {
    let mut p1d: Port<u8> = Port::new(PIC1_DATA);
    let mut p2d: Port<u8> = Port::new(PIC2_DATA);
    let mut p1c: Port<u8> = Port::new(PIC1_CMD);
    let mut p2c: Port<u8> = Port::new(PIC2_CMD);

    // Save masks (not used further but good practice).
    let _m1 = p1d.read();
    let _m2 = p2d.read();

    // Initialise cascade.
    p1c.write(ICW1_INIT);
    io_wait();
    p2c.write(ICW1_INIT);
    io_wait();

    // Set vector offsets.
    p1d.write(PIC1_OFFSET);
    io_wait();
    p2d.write(PIC2_OFFSET);
    io_wait();

    // Tell PIC1 slave is at IRQ2; tell PIC2 its cascade identity.
    p1d.write(0x04);
    io_wait();
    p2d.write(0x02);
    io_wait();

    // 8086 mode.
    p1d.write(ICW4_8086);
    io_wait();
    p2d.write(ICW4_8086);
    io_wait();

    // Mask everything on PIC2; on PIC1 unmask only IRQ0 (timer) + IRQ2
    // (cascade to PIC2 – required even if all PIC2 IRQs are masked).
    p1d.write(0b1111_1000); // IRQ0 + IRQ1 + IRQ2 unmasked
    p2d.write(0b1111_1111); // all PIC2 masked
}

/// Send end-of-interrupt to the appropriate PIC(s).
///
/// Must be called at the end of every hardware IRQ handler.
pub fn end_of_interrupt(irq_vector: u8) {
    // SAFETY: I/O port writes to the PIC.
    unsafe {
        if irq_vector >= PIC2_OFFSET {
            Port::<u8>::new(PIC2_CMD).write(0x20);
        }
        Port::<u8>::new(PIC1_CMD).write(0x20);
    }
}

/// One I/O cycle delay – lets slow ISA devices settle.
fn io_wait() {
    // Writing to port 0x80 is a standard I/O delay trick.
    unsafe { Port::<u8>::new(0x80).write(0) };
}
