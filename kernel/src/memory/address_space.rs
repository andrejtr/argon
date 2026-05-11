//! Per-process virtual address space.
//!
//! Each user process gets its own Level-4 page table (CR3).  The upper half
//! (indices 256–511) is a shallow copy of the kernel's L4 entries so that
//! interrupt/syscall handlers can run without a CR3 switch.  Lower half
//! entries belong exclusively to the process.
//!
//! Virtual memory layout for a user process:
//!   0x0040_0000                 ELF segments (default load address)
//!   USER_STACK_TOP – STACK_SIZE user stack (256 KiB, grows down)
//!   0x0000_7FFF_FFFF_0000       USER_STACK_TOP

use x86_64::{
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame,
        Size4KiB,
    },
    VirtAddr,
};

use crate::future::elf::LoadedElf;

/// User stack top virtual address.
pub const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;
/// Size of the user stack.
pub const USER_STACK_SIZE: u64 = 256 * 1024; // 256 KiB

/// A per-process address space backed by its own L4 page table.
pub struct UserAddressSpace {
    /// Physical frame holding the process's L4 page table.
    pub cr3: PhysFrame,
}

impl UserAddressSpace {
    /// Create a new address space.
    ///
    /// Allocates a fresh L4 table and copies kernel upper-half entries from
    /// the currently active page table.
    pub fn new() -> Option<Self> {
        let l4_frame = super::frame_alloc::alloc_frame()?;
        let l4_virt = super::phys_to_virt(l4_frame.start_address());

        // Zero the new L4 table.
        unsafe { core::ptr::write_bytes(l4_virt.as_mut_ptr::<u8>(), 0, 4096) };

        // Copy kernel upper-half mappings (indices 256–511) from active L4.
        let kernel_l4 = unsafe { active_l4_table() };
        let new_l4: &mut PageTable = unsafe { &mut *l4_virt.as_mut_ptr() };
        for i in 256..512 {
            new_l4[i] = kernel_l4[i].clone();
        }

        Some(UserAddressSpace { cr3: l4_frame })
    }

    /// Map a single ELF segment into this address space.
    ///
    /// `data` is the segment content (already zero-padded to `memsz` by the
    /// ELF loader).  Permissions follow the ELF `p_flags`.
    pub fn map_segment(
        &mut self,
        vaddr: u64,
        data: &[u8],
        writable: bool,
        executable: bool,
    ) -> bool {
        let page_count = data.len().div_ceil(4096);
        let mut mapper = self.mapper();

        for i in 0..page_count {
            let frame = match super::frame_alloc::alloc_frame() {
                Some(f) => f,
                None => return false,
            };
            // Write segment data into the frame.
            let frame_virt = super::phys_to_virt(frame.start_address());
            let chunk_start = i * 4096;
            let chunk_end = (chunk_start + 4096).min(data.len());
            unsafe {
                let dst = frame_virt.as_mut_ptr::<u8>();
                core::ptr::copy_nonoverlapping(
                    data[chunk_start..chunk_end].as_ptr(),
                    dst,
                    chunk_end - chunk_start,
                );
                // Zero any trailing BSS bytes in this page.
                core::ptr::write_bytes(
                    dst.add(chunk_end - chunk_start),
                    0,
                    4096 - (chunk_end - chunk_start),
                );
            }
            let page_vaddr = vaddr + (i as u64) * 4096;
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_vaddr));
            let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
            if writable {
                flags |= PageTableFlags::WRITABLE;
            }
            if !executable {
                flags |= PageTableFlags::NO_EXECUTE;
            }
            unsafe {
                if mapper.map_to(page, frame, flags, &mut FrameAlloc).is_err() {
                    return false;
                }
            }
        }
        true
    }

    /// Map the user stack (256 KiB, writable, NX).
    pub fn map_stack(&mut self) -> bool {
        let stack_bottom = USER_STACK_TOP - USER_STACK_SIZE;
        let page_count = (USER_STACK_SIZE / 4096) as usize;
        let mut mapper = self.mapper();
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::NO_EXECUTE;

        for i in 0..page_count {
            let frame = match super::frame_alloc::alloc_frame() {
                Some(f) => f,
                None => return false,
            };
            // Zero-fill user stack.
            let frame_virt = super::phys_to_virt(frame.start_address());
            unsafe { core::ptr::write_bytes(frame_virt.as_mut_ptr::<u8>(), 0, 4096) };

            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                stack_bottom + (i as u64) * 4096,
            ));
            unsafe {
                if mapper.map_to(page, frame, flags, &mut FrameAlloc).is_err() {
                    return false;
                }
            }
        }
        true
    }

    /// Load all segments from a parsed ELF into this address space.
    ///
    /// Returns the entry point on success.
    pub fn load_elf(&mut self, elf: &LoadedElf) -> Option<u64> {
        for seg in &elf.segments {
            if !self.map_segment(seg.vaddr, &seg.data, seg.writable, seg.executable) {
                return None;
            }
        }
        if !self.map_stack() {
            return None;
        }
        Some(elf.entry)
    }

    /// Build an `OffsetPageTable` over this address space's L4 table.
    fn mapper(&mut self) -> OffsetPageTable<'_> {
        let l4: &mut PageTable =
            unsafe { &mut *super::phys_to_virt(self.cr3.start_address()).as_mut_ptr() };
        unsafe { OffsetPageTable::new(l4, super::phys_offset()) }
    }
}

/// Helper to access the currently active L4 page table via the physical
/// memory offset mapping.
unsafe fn active_l4_table() -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;
    let (frame, _) = Cr3::read();
    let virt = super::phys_to_virt(frame.start_address());
    &mut *virt.as_mut_ptr()
}

/// Minimal `FrameAllocator` wrapper used by `map_to` to allocate
/// intermediate page-table frames.
struct FrameAlloc;

unsafe impl FrameAllocator<Size4KiB> for FrameAlloc {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = super::frame_alloc::alloc_frame()?;
        // Zero new intermediate tables.
        let virt = super::phys_to_virt(frame.start_address());
        unsafe { core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, 4096) };
        Some(frame)
    }
}
