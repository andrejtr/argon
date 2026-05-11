//! Block device abstraction layer.
//!
//! Provides a `BlockDevice` trait that every storage driver implements,
//! plus a global device registry so higher-level code (FAT32, etc.) can
//! enumerate and read from attached disks without knowing the underlying
//! hardware interface.

use alloc::{boxed::Box, vec::Vec};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Errors returned by block device operations.
#[derive(Debug)]
pub enum BlockError {
    /// Generic I/O failure (command failed, drive error, …).
    Io,
    /// Device did not respond within the expected polling window.
    Timeout,
    /// LBA range requested is beyond the device capacity.
    OutOfRange,
}

/// Trait implemented by every block device driver.
///
/// All operations work on 512-byte logical sectors.
pub trait BlockDevice: Send {
    /// Read `count` consecutive 512-byte sectors starting at `lba` into `buf`.
    ///
    /// `buf` must be exactly `count * 512` bytes long.
    fn read_blocks(&self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError>;

    /// Total number of 512-byte sectors the device exposes.
    ///
    /// May return `u64::MAX` if the capacity is unknown.
    fn block_count(&self) -> u64;
}

// ---------------------------------------------------------------------------
// Global device registry
// ---------------------------------------------------------------------------

static DEVICES: Mutex<Vec<Box<dyn BlockDevice>>> = Mutex::new(Vec::new());

/// Register a block device; returns the assigned device index.
pub fn register(dev: Box<dyn BlockDevice>) -> usize {
    let mut guard = DEVICES.lock();
    let idx = guard.len();
    guard.push(dev);
    drop(guard);
    crate::serial_println!("block: registered device #{}", idx);
    idx
}

/// Number of registered block devices.
pub fn device_count() -> usize {
    DEVICES.lock().len()
}

/// Read `count` 512-byte sectors from device `dev_idx` starting at `lba`.
///
/// `buf` must be exactly `count * 512` bytes long.
pub fn read_blocks(dev_idx: usize, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), BlockError> {
    let guard = DEVICES.lock();
    let dev = guard.get(dev_idx).ok_or(BlockError::Io)?;
    dev.read_blocks(lba, count, buf)
}
