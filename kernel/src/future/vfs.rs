//! VFS abstraction layer.
//!
//! Defines the core traits that every filesystem driver must implement and
//! a global `MountTable` that maps path prefixes to `FileSystem` backends.
//! Concrete implementations (RamFS, FAT, ext2-like) live in submodules.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Reverse;
use spin::Mutex;

/// An opaque file descriptor handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Fd(pub u32);

/// Result type for VFS operations.
pub type VfsResult<T> = Result<T, VfsError>;

#[derive(Debug)]
pub enum VfsError {
    NotFound,
    PermissionDenied,
    NotAFile,
    NotADirectory,
    Io,
    Unsupported,
    NoMount,
}

/// Core filesystem trait.
///
/// All methods take `&mut self` because most filesystem operations mutate
/// internal state (inode tables, allocation bitmaps, dirty flags, etc.).
pub trait FileSystem: Send {
    /// Open a file by path, returning a descriptor.
    fn open(&mut self, path: &str) -> VfsResult<Fd>;
    /// Read up to `buf.len()` bytes from `fd` into `buf`.
    fn read(&mut self, fd: Fd, buf: &mut [u8]) -> VfsResult<usize>;
    /// Write all bytes in `buf` to `fd`.
    fn write(&mut self, fd: Fd, buf: &[u8]) -> VfsResult<usize>;
    /// Close the file descriptor.
    fn close(&mut self, fd: Fd) -> VfsResult<()>;
    /// List entries inside a directory path.
    fn readdir(&mut self, path: &str) -> VfsResult<Vec<String>>;
}

// ---------------------------------------------------------------------------
// Global mount table
// ---------------------------------------------------------------------------

type BoxedFs = alloc::boxed::Box<dyn FileSystem>;

struct Mount {
    prefix: String,
    fs: BoxedFs,
}

/// Global VFS mount table.
pub static VFS: Mutex<MountTable> = Mutex::new(MountTable::new());

pub struct MountTable {
    mounts: Vec<Mount>,
}

impl MountTable {
    pub const fn new() -> Self {
        MountTable { mounts: Vec::new() }
    }

    /// Mount a filesystem at `prefix` (e.g. `"/"`, `"/proc"`, `"/mnt/disk"`).
    pub fn mount(&mut self, prefix: &str, fs: BoxedFs) {
        self.mounts.push(Mount {
            prefix: String::from(prefix),
            fs,
        });
        // Keep most-specific (longest prefix) first.
        self.mounts
            .sort_unstable_by_key(|m| Reverse(m.prefix.len()));
    }

    /// Find the mount index whose prefix best matches `path`, plus the
    /// sub-path inside that mount.
    fn find(&self, path: &str) -> Option<(usize, String)> {
        for (i, mount) in self.mounts.iter().enumerate() {
            if path.starts_with(mount.prefix.as_str()) {
                let rem = &path[mount.prefix.len()..];
                let inner = String::from(if rem.is_empty() { "/" } else { rem });
                return Some((i, inner));
            }
        }
        None
    }

    pub fn open(&mut self, path: &str) -> VfsResult<Fd> {
        let (idx, inner) = self.find(path).ok_or(VfsError::NoMount)?;
        self.mounts[idx].fs.open(&inner)
    }

    pub fn read(&mut self, fd: Fd, buf: &mut [u8]) -> VfsResult<usize> {
        // fd is mount-agnostic — iterate mounts and try each.
        for mount in &mut self.mounts {
            if let Ok(n) = mount.fs.read(fd, buf) {
                return Ok(n);
            }
        }
        Err(VfsError::NotFound)
    }

    pub fn write(&mut self, fd: Fd, buf: &[u8]) -> VfsResult<usize> {
        for mount in &mut self.mounts {
            if let Ok(n) = mount.fs.write(fd, buf) {
                return Ok(n);
            }
        }
        Err(VfsError::NotFound)
    }

    pub fn close(&mut self, fd: Fd) -> VfsResult<()> {
        for mount in &mut self.mounts {
            if mount.fs.close(fd).is_ok() {
                return Ok(());
            }
        }
        Err(VfsError::NotFound)
    }

    pub fn readdir(&mut self, path: &str) -> VfsResult<Vec<String>> {
        let (idx, inner) = self.find(path).ok_or(VfsError::NoMount)?;
        self.mounts[idx].fs.readdir(&inner)
    }
}
