//! Read-only FAT32 filesystem driver.
//!
//! Implements the kernel `FileSystem` trait so FAT32 volumes can be mounted
//! in the global VFS.  Only 8.3 short-name directory entries are interpreted;
//! Long File Name (LFN) entries are silently skipped.
//!
//! The driver probes every registered block device in order and mounts the
//! first valid FAT32 volume it finds.  Call `init()` after the block layer
//! is populated (i.e. after `drivers::ahci::init()`).

use alloc::{boxed::Box, string::String, vec, vec::Vec};

use crate::future::vfs::{Fd, FileSystem, VfsError, VfsResult};
use crate::serial_println;

// ---------------------------------------------------------------------------
// BPB layout constants (byte offsets in sector 0)
// ---------------------------------------------------------------------------
const OFF_BYTES_PER_SECTOR: usize = 11;
const OFF_SECTORS_PER_CLUSTER: usize = 13;
const OFF_RESERVED_SECTORS: usize = 14;
const OFF_NUM_FATS: usize = 16;
const OFF_FAT_SIZE_32: usize = 36;
const OFF_ROOT_CLUSTER: usize = 44;
const OFF_FS_TYPE: usize = 82; // "FAT32   " string

// FAT32 end-of-cluster marker
const FAT32_EOC: u32 = 0x0FFF_FFF8;

// Directory entry attribute bits
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_LONG_NAME: u8 = 0x0F; // LFN entry (all attr bits set in a specific combination)
const ATTR_VOLUME_ID: u8 = 0x08;

// ---------------------------------------------------------------------------
// Helper: read a little-endian u16 / u32 from a byte slice
// ---------------------------------------------------------------------------

fn le16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn le32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

// ---------------------------------------------------------------------------
// Open file state
// ---------------------------------------------------------------------------

struct OpenFile {
    /// Pre-resolved cluster chain (may be empty for zero-length files).
    clusters: Vec<u32>,
    /// Current byte offset within the file.
    offset: usize,
    /// Total file size in bytes.
    size: u32,
}

// ---------------------------------------------------------------------------
// Fat32Fs
// ---------------------------------------------------------------------------

pub struct Fat32Fs {
    /// Index into the global block-device registry.
    dev_idx: usize,
    /// LBA of the first sector of the FAT32 volume.
    part_start: u64,
    bytes_per_sector: u32,
    sectors_per_cluster: u32,
    /// LBA of FAT #0.
    fat_lba: u64,
    /// LBA of the data area (cluster 2).
    data_lba: u64,
    /// First cluster of the root directory.
    root_cluster: u32,
    /// Open file table indexed by Fd value.
    open: Vec<Option<OpenFile>>,
    /// Next Fd value to issue.
    next_fd: u32,
}

impl Fat32Fs {
    // ── Internal helpers ──────────────────────────────────────────────────

    /// Read one 512-byte sector.
    fn read_sector(&self, lba: u64, buf: &mut [u8; 512]) -> bool {
        crate::block::read_blocks(self.dev_idx, lba, 1, buf).is_ok()
    }

    /// Convert cluster number → first LBA of that cluster.
    fn cluster_to_lba(&self, cluster: u32) -> u64 {
        self.data_lba + (cluster as u64 - 2) * self.sectors_per_cluster as u64
    }

    /// Follow the FAT chain for `cluster`, returning all cluster numbers.
    fn cluster_chain(&self, start: u32) -> Vec<u32> {
        let mut chain = Vec::new();
        let mut cur = start;
        let mut buf = [0u8; 512];
        loop {
            if !(2..FAT32_EOC).contains(&cur) {
                break;
            }
            chain.push(cur);
            // Each FAT32 entry is 4 bytes; find which sector of the FAT it's in.
            let entry_byte = cur as u64 * 4;
            let fat_sector_off = entry_byte / self.bytes_per_sector as u64;
            let byte_in_sector = (entry_byte % self.bytes_per_sector as u64) as usize;
            let lba = self.fat_lba + fat_sector_off;
            if !self.read_sector(lba, &mut buf) {
                break;
            }
            let next = le32(&buf, byte_in_sector) & 0x0FFF_FFFF;
            cur = next;
        }
        chain
    }

    /// Read raw bytes from the cluster chain starting at `chain_offset`.
    fn read_from_chain(&self, clusters: &[u32], chain_offset: usize, dst: &mut [u8]) -> usize {
        let cluster_size = self.sectors_per_cluster as usize * 512;
        let mut cluster_idx = chain_offset / cluster_size;
        let mut byte_in_cluster = chain_offset % cluster_size;
        let mut total_read = 0;
        let mut buf = [0u8; 512];

        while total_read < dst.len() {
            if cluster_idx >= clusters.len() {
                break;
            }
            let cluster = clusters[cluster_idx];
            let sector_idx = byte_in_cluster / 512;
            let byte_in_sector = byte_in_cluster % 512;
            let lba = self.cluster_to_lba(cluster) + sector_idx as u64;
            if !self.read_sector(lba, &mut buf) {
                break;
            }
            let avail = 512 - byte_in_sector;
            let want = dst.len() - total_read;
            let copy_len = avail.min(want);
            dst[total_read..total_read + copy_len]
                .copy_from_slice(&buf[byte_in_sector..byte_in_sector + copy_len]);
            total_read += copy_len;
            byte_in_cluster += copy_len;
            if byte_in_cluster >= cluster_size {
                cluster_idx += 1;
                byte_in_cluster = 0;
            }
        }
        total_read
    }

    /// Parse a raw 8.3 directory entry name (bytes 0-10) into a trimmed string.
    ///
    /// Returns `None` for deleted, unused, or LFN entries.
    fn parse_83_name(entry: &[u8; 32]) -> Option<String> {
        let attr = entry[11];
        // Skip LFN entries and volume labels
        if attr == ATTR_LONG_NAME || (attr & ATTR_VOLUME_ID != 0 && attr & ATTR_DIRECTORY == 0) {
            return None;
        }
        // Skip free/deleted entries
        if entry[0] == 0x00 || entry[0] == 0xE5 {
            return None;
        }
        let name_bytes = &entry[0..8];
        let ext_bytes = &entry[8..11];

        // Build name part (trim trailing spaces)
        let name_end = name_bytes
            .iter()
            .rposition(|&b| b != b' ')
            .map_or(0, |i| i + 1);
        let ext_end = ext_bytes
            .iter()
            .rposition(|&b| b != b' ')
            .map_or(0, |i| i + 1);

        let mut s = String::new();
        for &b in &name_bytes[..name_end] {
            s.push(b.to_ascii_lowercase() as char);
        }
        if ext_end > 0 {
            s.push('.');
            for &b in &ext_bytes[..ext_end] {
                s.push(b.to_ascii_lowercase() as char);
            }
        }
        Some(s)
    }

    /// Find an entry named `name` (case-insensitive 8.3) inside directory
    /// `dir_clusters`.  Returns `(first_cluster, file_size, attr)`.
    fn find_in_dir(&self, dir_clusters: &[u32], name: &str) -> Option<(u32, u32, u8)> {
        let cluster_size = self.sectors_per_cluster as usize * 512;
        let mut raw = vec![0u8; cluster_size];

        for &cluster in dir_clusters {
            let base_lba = self.cluster_to_lba(cluster);
            let mut buf = [0u8; 512];
            for sec in 0..self.sectors_per_cluster as u64 {
                if !self.read_sector(base_lba + sec, &mut buf) {
                    return None;
                }
                let off = sec as usize * 512;
                raw[off..off + 512].copy_from_slice(&buf);
            }

            let entries = cluster_size / 32;
            for i in 0..entries {
                let entry: &[u8; 32] = raw[i * 32..(i + 1) * 32].try_into().ok()?;
                if entry[0] == 0x00 {
                    return None; // no more entries
                }
                if let Some(entry_name) = Self::parse_83_name(entry) {
                    if entry_name.eq_ignore_ascii_case(name) {
                        let attr = entry[11];
                        let cluster_hi = le16(entry, 20) as u32;
                        let cluster_lo = le16(entry, 26) as u32;
                        let first_cluster = (cluster_hi << 16) | cluster_lo;
                        let file_size = le32(entry, 28);
                        return Some((first_cluster, file_size, attr));
                    }
                }
            }
        }
        None
    }

    /// Walk `path` (e.g. `"bin/shell"`) starting from the root directory.
    /// Returns `(first_cluster, file_size, attr)` for the final component.
    fn walk_path(&self, path: &str) -> Option<(u32, u32, u8)> {
        // Strip leading slash if present
        let path = path.trim_start_matches('/');

        let mut cluster_chain = self.cluster_chain(self.root_cluster);
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            // Root directory — no cluster/size concept for open, handled in readdir
            return Some((self.root_cluster, 0, ATTR_DIRECTORY));
        }

        for (i, component) in components.iter().enumerate() {
            let (first_cluster, file_size, attr) = self.find_in_dir(&cluster_chain, component)?;
            if i == components.len() - 1 {
                return Some((first_cluster, file_size, attr));
            }
            // Must be a directory to continue descending
            if attr & ATTR_DIRECTORY == 0 {
                return None;
            }
            cluster_chain = self.cluster_chain(first_cluster);
        }
        None
    }

    // ── Constructor ───────────────────────────────────────────────────────

    /// Try to parse a FAT32 BPB from sector 0 of block device `dev_idx`.
    ///
    /// Returns `None` if the device does not contain a valid FAT32 volume.
    pub fn try_new(dev_idx: usize) -> Option<Self> {
        let mut sector = [0u8; 512];
        crate::block::read_blocks(dev_idx, 0, 1, &mut sector).ok()?;

        // Validate FAT32 signature
        if &sector[OFF_FS_TYPE..OFF_FS_TYPE + 5] != b"FAT32" {
            serial_println!("fat32: dev#{} not FAT32 (no signature)", dev_idx);
            return None;
        }

        let bytes_per_sector = le16(&sector, OFF_BYTES_PER_SECTOR) as u32;
        if bytes_per_sector != 512 {
            serial_println!(
                "fat32: dev#{} unsupported sector size {}",
                dev_idx,
                bytes_per_sector
            );
            return None;
        }

        let sectors_per_cluster = sector[OFF_SECTORS_PER_CLUSTER] as u32;
        let reserved_sectors = le16(&sector, OFF_RESERVED_SECTORS) as u64;
        let num_fats = sector[OFF_NUM_FATS] as u64;
        let fat_size_32 = le32(&sector, OFF_FAT_SIZE_32) as u64;
        let root_cluster = le32(&sector, OFF_ROOT_CLUSTER);

        let fat_lba = reserved_sectors;
        let data_lba = reserved_sectors + num_fats * fat_size_32;

        serial_println!(
            "fat32: dev#{} spc={} reserved={} fats={} fat_sz={} root_cl={} data_lba={}",
            dev_idx,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            fat_size_32,
            root_cluster,
            data_lba
        );

        Some(Fat32Fs {
            dev_idx,
            part_start: 0,
            bytes_per_sector,
            sectors_per_cluster,
            fat_lba,
            data_lba,
            root_cluster,
            open: Vec::new(),
            next_fd: 3, // 0/1/2 reserved for stdin/stdout/stderr
        })
    }
}

impl FileSystem for Fat32Fs {
    fn open(&mut self, path: &str) -> VfsResult<Fd> {
        let (first_cluster, file_size, attr) = self.walk_path(path).ok_or(VfsError::NotFound)?;

        if attr & ATTR_DIRECTORY != 0 {
            return Err(VfsError::NotAFile);
        }

        let clusters = self.cluster_chain(first_cluster);

        let fd_val = self.next_fd;
        self.next_fd += 1;

        // Grow open table if needed
        while self.open.len() <= fd_val as usize {
            self.open.push(None);
        }
        self.open[fd_val as usize] = Some(OpenFile {
            clusters,
            offset: 0,
            size: file_size,
        });

        serial_println!("fat32: open {:?} fd={} size={}", path, fd_val, file_size);
        Ok(Fd(fd_val))
    }

    fn read(&mut self, fd: Fd, buf: &mut [u8]) -> VfsResult<usize> {
        // Clone the immutable parts we need so we can release the &mut borrow on
        // self.open before calling read_from_chain (which also borrows self).
        let (clusters, offset, size) = {
            let slot = self.open.get(fd.0 as usize).ok_or(VfsError::NotFound)?;
            let of = slot.as_ref().ok_or(VfsError::NotFound)?;
            (of.clusters.clone(), of.offset, of.size)
        };

        let remaining = size as usize - offset;
        if remaining == 0 {
            return Ok(0);
        }
        let want = buf.len().min(remaining);
        let n = self.read_from_chain(&clusters, offset, &mut buf[..want]);

        // Update the offset now that we have no outstanding borrows.
        let slot = self.open.get_mut(fd.0 as usize).unwrap();
        let of = slot.as_mut().unwrap();
        of.offset += n;
        Ok(n)
    }

    fn write(&mut self, _fd: Fd, _buf: &[u8]) -> VfsResult<usize> {
        Err(VfsError::Unsupported) // read-only
    }

    fn close(&mut self, fd: Fd) -> VfsResult<()> {
        if let Some(slot) = self.open.get_mut(fd.0 as usize) {
            if slot.is_some() {
                *slot = None;
                return Ok(());
            }
        }
        Err(VfsError::NotFound)
    }

    fn readdir(&mut self, path: &str) -> VfsResult<Vec<String>> {
        let dir_cluster = if path == "/" || path.is_empty() {
            self.root_cluster
        } else {
            let (first_cluster, _, attr) = self.walk_path(path).ok_or(VfsError::NotFound)?;
            if attr & ATTR_DIRECTORY == 0 {
                return Err(VfsError::NotADirectory);
            }
            first_cluster
        };

        let dir_chain = self.cluster_chain(dir_cluster);
        let cluster_size = self.sectors_per_cluster as usize * 512;
        let mut names: Vec<String> = Vec::new();
        let mut raw = vec![0u8; cluster_size];
        let mut buf = [0u8; 512];

        'outer: for &cluster in &dir_chain {
            let base_lba = self.cluster_to_lba(cluster);
            for sec in 0..self.sectors_per_cluster as u64 {
                if !self.read_sector(base_lba + sec, &mut buf) {
                    break 'outer;
                }
                let off = sec as usize * 512;
                raw[off..off + 512].copy_from_slice(&buf);
            }
            let entries = cluster_size / 32;
            for i in 0..entries {
                let entry: &[u8; 32] = match raw[i * 32..(i + 1) * 32].try_into() {
                    Ok(e) => e,
                    Err(_) => break 'outer,
                };
                if entry[0] == 0x00 {
                    break 'outer;
                }
                if let Some(name) = Self::parse_83_name(entry) {
                    // Skip . and ..
                    if name != "." && name != ".." {
                        names.push(name);
                    }
                }
            }
        }
        Ok(names)
    }
}

// ---------------------------------------------------------------------------
// Module-level init
// ---------------------------------------------------------------------------

/// Probe registered block devices for a FAT32 volume and, if found, mount it
/// at `"/"` in the global VFS (replacing any existing mount there).
///
/// Returns `true` if a FAT32 volume was successfully mounted.
pub fn init() -> bool {
    let count = crate::block::device_count();
    for dev_idx in 0..count {
        if let Some(fs) = Fat32Fs::try_new(dev_idx) {
            serial_println!("fat32: mounting dev#{} at /", dev_idx);
            crate::future::vfs::VFS.lock().mount("/", Box::new(fs));
            serial_println!("fat32: mounted OK");
            return true;
        }
    }
    serial_println!("fat32: no FAT32 volume found ({} devices probed)", count);
    false
}
