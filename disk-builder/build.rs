/// build.rs for disk-builder.
///
/// Uses the `bootloader` crate's `BiosBoot` and `UefiBoot` builders to
/// create raw disk images from the compiled kernel binary.
///
/// Also creates a `data.img` FAT32 volume containing the userland binaries
/// so that the kernel can load programs from disk at runtime (M3).
///
/// After `cargo build -p disk-builder` completes you will find:
///   target/disk-builder-out/bios.img   ← attach this to VirtualBox (raw)
///   target/disk-builder-out/uefi.img   ← or this for UEFI boot
///   target/disk-builder-out/data.img   ← FAT32 data disk with /bin/shell
///
/// All three paths are exported as Cargo env-vars so `src/main.rs` can
/// print them and pass them to QEMU.
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::{env, fs};

fn main() {
    // Kernel binary path injected by the artifact dependency.
    let kernel = PathBuf::from(
        env::var_os("CARGO_BIN_FILE_KERNEL_kernel")
            .expect("CARGO_BIN_FILE_KERNEL_kernel not set – run with `--package disk-builder`"),
    );

    // Shell binary path injected by the artifact dependency.
    let shell = PathBuf::from(
        env::var_os("CARGO_BIN_FILE_SHELL_shell")
            .expect("CARGO_BIN_FILE_SHELL_shell not set"),
    );

    // Put the disk images in a deterministic, human-readable location.
    let out_dir = {
        let workspace_root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .parent()
            .unwrap()
            .to_path_buf();
        workspace_root.join("target").join("disk-builder-out")
    };
    fs::create_dir_all(&out_dir).expect("failed to create output directory");

    // --- BIOS disk image ---
    let bios_path = out_dir.join("bios.img");
    bootloader::BiosBoot::new(&kernel)
        .create_disk_image(&bios_path)
        .expect("failed to create BIOS disk image");

    // --- UEFI disk image ---
    let uefi_path = out_dir.join("uefi.img");
    bootloader::UefiBoot::new(&kernel)
        .create_disk_image(&uefi_path)
        .expect("failed to create UEFI disk image");

    // --- FAT32 data image ---
    let data_path = out_dir.join("data.img");
    create_fat32_image(&data_path, &shell).expect("failed to create FAT32 data image");

    // Export paths so src/main.rs can print them via env!().
    println!("cargo:rustc-env=ARGON_BIOS_IMAGE={}", bios_path.display());
    println!("cargo:rustc-env=ARGON_UEFI_IMAGE={}", uefi_path.display());
    println!("cargo:rustc-env=ARGON_DATA_IMAGE={}", data_path.display());

    // Re-run build.rs if the kernel or shell binary changes.
    println!("cargo:rerun-if-changed={}", kernel.display());
    println!("cargo:rerun-if-changed={}", shell.display());
}

// ---------------------------------------------------------------------------
// FAT32 image creation
// ---------------------------------------------------------------------------

/// Create a raw FAT32 volume at `path` and populate it with userland
/// binaries and OS metadata files.
///
/// The image is 64 MiB, which is large enough for all current binaries.
/// No MBR partition table is used — the FAT32 BPB starts at sector 0 so
/// the kernel can parse it directly.
fn create_fat32_image(path: &PathBuf, shell_bin: &PathBuf) -> std::io::Result<()> {
    const IMAGE_SIZE: u64 = 8 * 1024 * 1024; // 8 MiB – fits the shell ELF with room to spare

    // Create or truncate the image file and pre-allocate space.
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.set_len(IMAGE_SIZE)?;

    // Format as FAT32.
    let opts = fatfs::FormatVolumeOptions::new()
        .volume_label(*b"ARGON      ")
        .fat_type(fatfs::FatType::Fat32);
    fatfs::format_volume(&mut file, opts)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("fatfs format: {e}")))?;

    // Seek back to start before opening the formatted volume.
    file.seek(SeekFrom::Start(0))?;

    // Open the formatted volume and populate it.
    let fs = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("fatfs open: {e}")))?;
    let root = fs.root_dir();

    // /BIN/SHELL — the ring-3 shell binary.
    {
        root.create_dir("BIN")
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("mkdir BIN: {e}")))?;
        let bin = root.open_dir("BIN")
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("open BIN: {e}")))?;
        let shell_bytes = fs::read(shell_bin)?;
        let mut f = bin.create_file("SHELL")
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("create SHELL: {e}")))?;
        f.write_all(&shell_bytes)?;
    }

    // /ETC/OS-RELE (8.3 for os-release)
    {
        root.create_dir("ETC")
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("mkdir ETC: {e}")))?;
        let etc = root.open_dir("ETC")
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("open ETC: {e}")))?;
        let mut f = etc.create_file("OS-RELE")
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("create OS-RELE: {e}")))?;
        f.write_all(b"NAME=argonOS\nVERSION=0.2.0\n")?;
    }

    // /BOOT/MOTD
    {
        root.create_dir("BOOT")
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("mkdir BOOT: {e}")))?;
        let boot = root.open_dir("BOOT")
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("open BOOT: {e}")))?;
        let mut f = boot.create_file("MOTD")
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("create MOTD: {e}")))?;
        f.write_all(b"Welcome to argonOS!\n")?;
    }

    println!("cargo:warning=data.img: wrote BIN/SHELL ({} bytes), ETC/OS-RELE, BOOT/MOTD",
        fs::metadata(shell_bin).map(|m| m.len()).unwrap_or(0));

    Ok(())
}
