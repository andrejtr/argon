//! Global physical frame bump allocator.
//!
//! Wraps the bootloader-provided `BootInfoFrameAllocator` in a `Mutex` so it
//! can be called from anywhere after `init()`.  We never free frames (bump
//! allocator), which is fine for the initial bring-up; a proper buddy
//! allocator can replace this without changing the public API.

use bootloader_api::info::MemoryRegions;
use spin::Mutex;
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};

use super::allocator::BootInfoFrameAllocator;

static FRAME_ALLOC: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);

/// Initialise the global frame allocator from the bootloader memory map.
///
/// # Safety
/// Must be called exactly once with the genuine bootloader memory regions.
pub unsafe fn init(regions: &'static MemoryRegions) {
    *FRAME_ALLOC.lock() = Some(BootInfoFrameAllocator::new(regions));
}

/// Allocate a single 4 KiB physical frame.
///
/// Returns `None` if physical memory is exhausted.
pub fn alloc_frame() -> Option<PhysFrame<Size4KiB>> {
    FRAME_ALLOC.lock().as_mut()?.allocate_frame()
}
