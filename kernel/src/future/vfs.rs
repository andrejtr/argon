//! VFS abstraction layer.
//!
//! Defines the core traits that every filesystem driver must implement.
//! Concrete implementations (RamFS, FAT, ext2-like) will live in submodules.

/// An opaque file descriptor handle.
///
/// Wrapping an integer in a newtype lets the type system prevent mixing up
/// file descriptors from different contexts.
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
}

/// Core filesystem trait.
///
/// All methods take `&mut self` because most filesystem operations mutate
/// internal state (inode tables, allocation bitmaps, dirty flags, etc.).
pub trait FileSystem {
    /// Open a file by path, returning a descriptor.
    fn open(&mut self, path: &str) -> VfsResult<Fd>;

    /// Read up to `buf.len()` bytes from `fd` into `buf`.
    /// Returns the number of bytes actually read.
    fn read(&mut self, fd: Fd, buf: &mut [u8]) -> VfsResult<usize>;

    /// Write all bytes in `buf` to `fd`.
    /// Returns the number of bytes written.
    fn write(&mut self, fd: Fd, buf: &[u8]) -> VfsResult<usize>;

    /// Close the file descriptor, releasing any associated resources.
    fn close(&mut self, fd: Fd) -> VfsResult<()>;

    /// List the names of entries inside a directory.
    fn readdir<'a>(&'a mut self, path: &str) -> VfsResult<&'a [&'a str]>;
}
