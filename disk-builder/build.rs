/// build.rs for disk-builder.
///
/// Uses the `bootloader` crate's `BiosBoot` and `UefiBoot` builders to
/// create raw disk images from the compiled kernel binary.
///
/// After `cargo build -p disk-builder` completes you will find:
///   target/disk-builder-out/bios.img   ← attach this to VirtualBox (raw)
///   target/disk-builder-out/uefi.img   ← or this for UEFI boot
///
/// Both paths are exported as Cargo env-vars so `src/main.rs` can print them.
use std::path::PathBuf;
use std::{env, fs};

fn main() {
    // Kernel binary path injected by the artifact dependency.
    let kernel = PathBuf::from(
        env::var_os("CARGO_BIN_FILE_KERNEL_kernel")
            .expect("CARGO_BIN_FILE_KERNEL_kernel not set – run with `--package disk-builder`"),
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

    // Export paths so src/main.rs can print them via env!().
    println!("cargo:rustc-env=ARGON_BIOS_IMAGE={}", bios_path.display());
    println!("cargo:rustc-env=ARGON_UEFI_IMAGE={}", uefi_path.display());

    // Re-run build.rs if the kernel binary changes.
    println!("cargo:rerun-if-changed={}", kernel.display());
}
