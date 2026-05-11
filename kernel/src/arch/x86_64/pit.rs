/// Programmable Interval Timer (PIT) — channel 0, mode 2 (rate generator).
///
/// Fires IRQ0 at `TICK_HZ` Hz.  The tick counter is read by the scheduler to
/// implement basic time-slicing.
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

const PIT_CHANNEL0: u16 = 0x40;
const PIT_CMD: u16 = 0x43;
const PIT_BASE_HZ: u32 = 1_193_182;

/// Desired tick rate (100 Hz → 10 ms per tick).
pub const TICK_HZ: u32 = 100;

/// Total ticks since boot.
pub static TICKS: AtomicU64 = AtomicU64::new(0);

/// Increment the tick counter (called from the timer IRQ handler).
#[inline]
pub fn tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Returns the current tick count.
#[inline]
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Program channel 0 for the target frequency.
///
/// # Safety
/// Must be called before enabling hardware interrupts.
pub unsafe fn init() {
    let divisor = (PIT_BASE_HZ / TICK_HZ) as u16;

    let mut cmd: Port<u8> = Port::new(PIT_CMD);
    let mut ch0: Port<u8> = Port::new(PIT_CHANNEL0);

    // Channel 0, lobyte/hibyte access, mode 2 (rate generator), binary.
    cmd.write(0x34);
    ch0.write((divisor & 0xFF) as u8);
    ch0.write((divisor >> 8) as u8);
}
