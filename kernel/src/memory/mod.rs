pub mod address_space;
pub mod allocator;
pub mod frame_alloc;
pub mod heap;
pub mod paging;
pub mod uaccess;

use core::sync::atomic::{AtomicU64, Ordering};

use bootloader_api::info::MemoryRegions;
use x86_64::{PhysAddr, VirtAddr};

use crate::serial_println;

/// Physical-to-virtual offset, set once during `init()`.
static PHYS_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Return the physical-memory mapping offset as a `VirtAddr`.
pub fn phys_offset() -> VirtAddr {
    VirtAddr::new(PHYS_OFFSET.load(Ordering::Relaxed))
}

/// Convert a physical address to its kernel virtual alias.
pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    phys_offset() + phys.as_u64()
}

/// Initialise the memory subsystem.
///
/// Must be called before any heap allocation or page-table manipulation.
/// `regions` must be `'static` (as provided by the bootloader).
pub fn init(phys_mem_offset: u64, regions: &'static MemoryRegions) {
    PHYS_OFFSET.store(phys_mem_offset, Ordering::Relaxed);
    let phys_offset_addr = VirtAddr::new(phys_mem_offset);

    // SAFETY: physical_memory_offset is trusted bootloader data.
    let _mapper = unsafe { paging::init(phys_offset_addr) };

    // SAFETY: regions is a valid bootloader memory map.
    unsafe { frame_alloc::init(regions) };

    heap::init();

    serial_println!("memory: physical offset {:#x}", phys_mem_offset);
}
