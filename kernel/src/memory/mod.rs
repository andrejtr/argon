pub mod allocator;
pub mod heap;
pub mod paging;
use crate::serial_println;

use x86_64::VirtAddr;

/// Initialise the memory subsystem.
///
/// Takes only the physical memory offset (a plain `u64`) extracted from
/// `BootInfo` by the caller.  This avoids permanently borrowing the entire
/// `BootInfo`, leaving the framebuffer field free for the display subsystem.
pub fn init(phys_mem_offset: u64) {
    let phys_offset = VirtAddr::new(phys_mem_offset);

    // SAFETY: physical_memory_offset is trusted bootloader data.
    let _mapper = unsafe { paging::init(phys_offset) };

    heap::init();

    serial_println!("memory: physical offset {:#x}", phys_mem_offset);
}
