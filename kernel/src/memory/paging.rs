/// Paging setup.
///
/// The bootloader maps *all* physical memory at a fixed virtual offset
/// (`BootInfo::physical_memory_offset`).  We wrap the active level-4 page
/// table in an `OffsetPageTable` which gives us safe, Rust-typed access to
/// the page tables without needing recursive mapping.
///
/// W^X invariant: every mapping created by this kernel must set *either*
/// `WRITABLE` *or* allow execution, never both simultaneously.
use x86_64::{
    structures::paging::{OffsetPageTable, PageTable},
    VirtAddr,
};

/// Construct an `OffsetPageTable` from the active CR3 + the bootloader's
/// physical-memory offset.
///
/// # Safety
/// * `physical_memory_offset` must be the value provided by the bootloader.
/// * Must be called only once; the returned table holds a mutable reference
///   to the live page tables.
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let l4 = active_level4_table(physical_memory_offset);
    OffsetPageTable::new(l4, physical_memory_offset)
}

/// Read CR3 and return a mutable reference to the active level-4 page table.
///
/// # Safety
/// The caller must ensure `physical_memory_offset` correctly maps all physical
/// addresses into virtual space.
unsafe fn active_level4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (l4_frame, _) = Cr3::read();
    let phys = l4_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let ptr: *mut PageTable = virt.as_mut_ptr();
    &mut *ptr
}

/// Translate a kernel virtual address to its physical counterpart.
///
/// Returns `None` if the address is not mapped.
pub fn translate_addr(
    addr: VirtAddr,
    physical_memory_offset: VirtAddr,
) -> Option<x86_64::PhysAddr> {
    use x86_64::structures::paging::mapper::TranslateResult;
    use x86_64::structures::paging::Translate;

    // SAFETY: We call this with the known-good offset from the bootloader.
    let mapper = unsafe { init(physical_memory_offset) };
    match mapper.translate(addr) {
        TranslateResult::Mapped { frame, offset, .. } => Some(frame.start_address() + offset),
        _ => None,
    }
}
