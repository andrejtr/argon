/// Kernel heap – 1 MiB static bump-region backed by linked_list_allocator.
///
/// The allocator lives on a static byte array so we never need to map extra
/// pages.  This is enough for the scheduler's process table, RamFS file
/// contents, and general kernel bookkeeping.
use linked_list_allocator::LockedHeap;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const HEAP_SIZE: usize = 1024 * 1024; // 1 MiB

// SAFETY: only touched by `init()` under the allocator's internal lock.
static mut HEAP_STORAGE: [u8; HEAP_SIZE] = [0u8; HEAP_SIZE];

/// Initialise the global allocator.  Must be called exactly once, before any
/// `alloc` operation.
pub fn init() {
    // SAFETY: Called once before any alloc; no other references to HEAP_STORAGE exist.
    #[allow(static_mut_refs)]
    let (ptr, len) = unsafe { (HEAP_STORAGE.as_mut_ptr(), HEAP_SIZE) };
    // SAFETY: ptr is valid for `len` bytes and will live for 'static.
    unsafe { ALLOCATOR.lock().init(ptr, len) };
}
