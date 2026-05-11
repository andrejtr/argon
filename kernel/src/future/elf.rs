//! Minimal ELF-64 loader.
//!
//! Parses a statically-linked, position-independent ELF binary and loads its
//! `PT_LOAD` segments into the target address space.  Only the subset of the
//! ELF spec required to run a simple Rust `no_std` userspace binary is
//! implemented.
//!
//! Limitations (to be lifted as the kernel matures):
//!   * Only ELF64 little-endian x86-64 (`EM_X86_64 = 62`) is supported.
//!   * Only `ET_EXEC` (static executable) and `ET_DYN` (PIE) types.
//!   * No dynamic linking / shared libraries.
//!   * Segments are currently copied into the *kernel* address space so that
//!     the initial bringup can be tested without a per-process page table.
//!     `load_into_process()` will map them into the process's address space
//!     once ring-3 processes land.

use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// ELF-64 on-disk structures (all little-endian)
// ---------------------------------------------------------------------------

const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1; // little-endian
const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 0x1; // segment execute permission
const PF_W: u32 = 0x2; // segment write permission
const PF_R: u32 = 0x4; // segment read permission

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Elf64Hdr {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Elf64Phdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

// ---------------------------------------------------------------------------
// Public API types
// ---------------------------------------------------------------------------

/// A segment that the loader has prepared for mapping into memory.
#[derive(Debug)]
pub struct LoadedSegment {
    /// Target virtual address (from ELF `p_vaddr`).
    pub vaddr: u64,
    /// Segment data (padded with zeros to `p_memsz`).
    pub data: Vec<u8>,
    /// Is this segment executable?
    pub executable: bool,
    /// Is this segment writable?
    pub writable: bool,
}

/// Result of a successful ELF load.
#[derive(Debug)]
pub struct LoadedElf {
    /// Virtual address of the program entry point.
    pub entry: u64,
    /// Loaded segments (to be mapped into the target address space).
    pub segments: Vec<LoadedSegment>,
}

/// ELF loading error.
#[derive(Debug)]
pub enum ElfError {
    /// The binary is too short to contain the ELF header.
    TooShort,
    /// The ELF magic bytes are missing or wrong.
    BadMagic,
    /// The ELF class is not 64-bit.
    NotElf64,
    /// The ELF data encoding is not little-endian.
    NotLittleEndian,
    /// The machine type is not x86-64.
    WrongArch,
    /// The ELF type is not executable or PIE.
    UnsupportedType,
    /// A program-header offset or size is out of bounds.
    OutOfBounds,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

/// Parse and load an ELF-64 binary from a byte slice.
///
/// Returns a [`LoadedElf`] describing the entry point and all loadable
/// segments.  The caller is responsible for mapping those segments into the
/// target address space.
///
/// # Errors
/// Returns an [`ElfError`] if the binary is malformed or unsupported.
pub fn load(bytes: &[u8]) -> Result<LoadedElf, ElfError> {
    // Validate minimum size.
    if bytes.len() < core::mem::size_of::<Elf64Hdr>() {
        return Err(ElfError::TooShort);
    }

    // Read the ELF header.
    // SAFETY: we just checked `bytes` is large enough and `Elf64Hdr` is
    // `repr(C, packed)` so unaligned reads are fine.
    let hdr: Elf64Hdr = unsafe { read_unaligned(bytes.as_ptr() as *const Elf64Hdr) };

    // Magic.
    if hdr.e_ident[..4] != ELF_MAGIC {
        return Err(ElfError::BadMagic);
    }
    if hdr.e_ident[4] != ELFCLASS64 {
        return Err(ElfError::NotElf64);
    }
    if hdr.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::NotLittleEndian);
    }
    let e_type = u16::from_le(hdr.e_type);
    if e_type != ET_EXEC && e_type != ET_DYN {
        return Err(ElfError::UnsupportedType);
    }
    if u16::from_le(hdr.e_machine) != EM_X86_64 {
        return Err(ElfError::WrongArch);
    }

    let entry = u64::from_le(hdr.e_entry);
    let phoff = u64::from_le(hdr.e_phoff) as usize;
    let phentsize = u16::from_le(hdr.e_phentsize) as usize;
    let phnum = u16::from_le(hdr.e_phnum) as usize;

    let mut segments = Vec::new();

    for i in 0..phnum {
        let ph_start = phoff
            .checked_add(i.checked_mul(phentsize).ok_or(ElfError::OutOfBounds)?)
            .ok_or(ElfError::OutOfBounds)?;
        let ph_end = ph_start
            .checked_add(core::mem::size_of::<Elf64Phdr>())
            .ok_or(ElfError::OutOfBounds)?;
        if ph_end > bytes.len() {
            return Err(ElfError::OutOfBounds);
        }

        // SAFETY: bounds checked above.
        let ph: Elf64Phdr =
            unsafe { read_unaligned(bytes[ph_start..].as_ptr() as *const Elf64Phdr) };

        if u32::from_le(ph.p_type) != PT_LOAD {
            continue;
        }

        let offset = u64::from_le(ph.p_offset) as usize;
        let filesz = u64::from_le(ph.p_filesz) as usize;
        let memsz = u64::from_le(ph.p_memsz) as usize;
        let vaddr = u64::from_le(ph.p_vaddr);
        let flags = u32::from_le(ph.p_flags);

        let src_end = offset.checked_add(filesz).ok_or(ElfError::OutOfBounds)?;
        if src_end > bytes.len() {
            return Err(ElfError::OutOfBounds);
        }

        // Copy filesz bytes, then zero-pad to memsz (BSS).
        let mut data: Vec<u8> = Vec::with_capacity(memsz);
        data.extend_from_slice(&bytes[offset..offset + filesz]);
        data.resize(memsz, 0u8);

        segments.push(LoadedSegment {
            vaddr,
            data,
            executable: (flags & PF_X) != 0,
            writable: (flags & PF_W) != 0,
        });
    }

    Ok(LoadedElf { entry, segments })
}

/// Read an unaligned `T` from a raw pointer.
///
/// # Safety
/// The pointed-to memory must be readable and large enough for `T`.
unsafe fn read_unaligned<T: Copy>(ptr: *const T) -> T {
    core::ptr::read_unaligned(ptr)
}
