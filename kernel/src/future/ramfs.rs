/// RamFS — a simple in-memory filesystem.
///
/// Stores up to `MAX_FILES` files, each up to `MAX_FILE_SIZE` bytes.
/// Implements the `FileSystem` trait so it can be mounted at any VFS path.
///
/// File descriptors are assigned sequentially and are valid until `close()`.
use alloc::{string::String, vec::Vec};

use crate::future::vfs::{Fd, FileSystem, VfsError, VfsResult};

/// Maximum number of files in the RAM filesystem.
const MAX_FILES: usize = 64;
/// Maximum size of a single file (64 KiB).
const MAX_FILE_SIZE: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

struct Inode {
    name: String,
    data: Vec<u8>,
}

struct OpenFile {
    inode_idx: usize,
    /// Current read/write cursor.
    offset: usize,
}

pub struct RamFs {
    inodes: Vec<Inode>,
    /// Open file table: index = Fd value, None = closed.
    open: Vec<Option<OpenFile>>,
    next_fd: u32,
}

impl RamFs {
    /// Create an empty RamFS instance.
    pub fn new() -> Self {
        RamFs {
            inodes: Vec::new(),
            open: Vec::new(),
            next_fd: 3, // 0=stdin, 1=stdout, 2=stderr reserved
        }
    }

    /// Create or overwrite a file with the given contents.
    pub fn create(&mut self, path: &str, data: &[u8]) -> VfsResult<()> {
        if data.len() > MAX_FILE_SIZE {
            return Err(VfsError::Io);
        }
        if self.inodes.len() >= MAX_FILES {
            return Err(VfsError::Io);
        }
        // Replace if exists.
        for inode in &mut self.inodes {
            if inode.name == path {
                inode.data = data.to_vec();
                return Ok(());
            }
        }
        self.inodes.push(Inode {
            name: String::from(path),
            data: data.to_vec(),
        });
        Ok(())
    }

    fn find_inode(&self, path: &str) -> Option<usize> {
        self.inodes.iter().position(|i| i.name == path)
    }

    fn alloc_fd(&mut self, inode_idx: usize) -> Fd {
        let fd_val = self.next_fd;
        self.next_fd += 1;
        let fd = Fd(fd_val);
        // Grow open table if needed.
        let idx = fd_val as usize;
        if self.open.len() <= idx {
            self.open.resize_with(idx + 1, || None);
        }
        self.open[idx] = Some(OpenFile {
            inode_idx,
            offset: 0,
        });
        fd
    }

    fn get_open(&mut self, fd: Fd) -> VfsResult<&mut OpenFile> {
        self.open
            .get_mut(fd.0 as usize)
            .and_then(|s| s.as_mut())
            .ok_or(VfsError::NotFound)
    }
}

impl FileSystem for RamFs {
    fn open(&mut self, path: &str) -> VfsResult<Fd> {
        let idx = self.find_inode(path).ok_or(VfsError::NotFound)?;
        Ok(self.alloc_fd(idx))
    }

    fn read(&mut self, fd: Fd, buf: &mut [u8]) -> VfsResult<usize> {
        // Extract index + offset without holding a borrow on self.open longer
        // than necessary, so we can then borrow self.inodes immutably.
        let (inode_idx, offset) = {
            let of = self.get_open(fd)?;
            (of.inode_idx, of.offset)
        };
        let data = &self.inodes[inode_idx].data;
        let available = data.len().saturating_sub(offset);
        let n = buf.len().min(available);
        buf[..n].copy_from_slice(&data[offset..offset + n]);
        // Update cursor.
        if let Some(Some(of)) = self.open.get_mut(fd.0 as usize) {
            of.offset += n;
        }
        Ok(n)
    }

    fn write(&mut self, fd: Fd, buf: &[u8]) -> VfsResult<usize> {
        let (inode_idx, offset) = {
            let of = self.get_open(fd)?;
            (of.inode_idx, of.offset)
        };
        let end = offset + buf.len();
        if end > MAX_FILE_SIZE {
            return Err(VfsError::Io);
        }
        let data = &mut self.inodes[inode_idx].data;
        if end > data.len() {
            data.resize(end, 0);
        }
        data[offset..end].copy_from_slice(buf);
        if let Some(Some(of)) = self.open.get_mut(fd.0 as usize) {
            of.offset = end;
        }
        Ok(buf.len())
    }

    fn close(&mut self, fd: Fd) -> VfsResult<()> {
        let slot = self.open.get_mut(fd.0 as usize).ok_or(VfsError::NotFound)?;
        if slot.is_none() {
            return Err(VfsError::NotFound);
        }
        *slot = None;
        Ok(())
    }

    fn readdir<'a>(&'a mut self, path: &str) -> VfsResult<&'a [&'a str]> {
        // RamFS is flat — "directory" is a prefix match.  For a flat FS the
        // root "/" contains everything; anything else is unsupported.
        if path != "/" {
            return Err(VfsError::NotADirectory);
        }
        // We can't easily return a slice of &str that borrows self here without
        // allocating; return Unsupported and let callers use `inodes` directly.
        Err(VfsError::Unsupported)
    }
}
