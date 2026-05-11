/// Physical frame allocator backed by the bootloader memory map.
///
/// The bootloader passes `BootInfo::memory_regions` which describes the entire
/// physical address space.  We iterate over `USABLE` regions and hand out
/// 4 KiB frames one at a time.  This is a simple, non-freeing bump allocator –
/// sufficient for the boot-time initialisation phase.
use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use x86_64::structures::paging::{FrameAllocator, PhysFrame, Size4KiB};
use x86_64::PhysAddr;

pub struct BootInfoFrameAllocator {
    regions: &'static MemoryRegions,
    next:    usize,
}

impl BootInfoFrameAllocator {
    /// Create the allocator.
    ///
    /// # Safety
    /// `regions` must be the genuine memory map from the bootloader and must
    /// remain valid for the lifetime of this allocator.
    pub unsafe fn new(regions: &'static MemoryRegions) -> Self {
        Self { regions, next: 0 }
    }

    /// Iterate over all usable 4 KiB-aligned frames in the memory map.
    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> + '_ {
        self.regions
            .iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable)
            .flat_map(|r| {
                let start = r.start;
                let end   = r.end;
                // Align start up and end down to 4 KiB boundaries.
                let frame_start = (start + 0xFFF) & !0xFFF;
                let frame_end   = end & !0xFFF;
                (frame_start..frame_end)
                    .step_by(4096)
                    .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
            })
    }
}

// SAFETY: We guarantee single-threaded boot access at this stage.
unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}
