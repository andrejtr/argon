/// disk-builder host binary.
///
/// After running `cargo build -p disk-builder` (or `--release`), this binary
/// prints the paths to the generated disk images and optionally boots them
/// in QEMU for rapid iteration.
///
/// Usage:
///   cargo build -p disk-builder              # build images
///   cargo run   -p disk-builder              # build images + launch QEMU (BIOS)
///   cargo run   -p disk-builder -- --uefi    # build images + launch QEMU (UEFI)
///
/// VirtualBox users:
///   1. The BIOS image path is printed to stdout.
///   2. Convert to VMDK:  qemu-img convert -f raw -O vmdk bios.img bios.vmdk
///   3. Create a new VM (Other Linux 64-bit), attach bios.vmdk as the hard disk.
///   4. Boot.

const BIOS_IMAGE: &str = env!("ARGON_BIOS_IMAGE");
const UEFI_IMAGE: &str = env!("ARGON_UEFI_IMAGE");

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let use_uefi = args.iter().any(|a| a == "--uefi");

    println!("╔══════════════════════════════════════════════════╗");
    println!("║              argonOS  disk-builder               ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();
    println!("  BIOS image : {}", BIOS_IMAGE);
    println!("  UEFI image : {}", UEFI_IMAGE);
    println!();

    if use_uefi {
        println!("  → Launching QEMU (UEFI) …");
        launch_qemu_uefi();
    } else {
        println!("  → Launching QEMU (BIOS) …");
        launch_qemu_bios();
    }
}

fn launch_qemu_bios() {
    let status = std::process::Command::new("qemu-system-x86_64")
        .args([
            "-drive",
            &format!("format=raw,file={BIOS_IMAGE}"),
            "-m",
            "512M",
            "-serial",
            "stdio",
            "-display",
            "gtk",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("QEMU exited with status: {s}"),
        Err(e) => {
            eprintln!("Could not launch QEMU: {e}");
            eprintln!("Install QEMU or load {BIOS_IMAGE} manually in VirtualBox.");
            print_virtualbox_instructions(BIOS_IMAGE);
        }
    }
}

fn launch_qemu_uefi() {
    // OVMF firmware is needed for UEFI.  The `ovmf-prebuilt` crate can provide
    // it, but for now we tell the user where to find it.
    let status = std::process::Command::new("qemu-system-x86_64")
        .args([
            "-drive",
            &format!("format=raw,file={UEFI_IMAGE}"),
            "-bios",
            "/usr/share/OVMF/OVMF_CODE.fd",
            "-m",
            "512M",
            "-serial",
            "stdio",
            "-display",
            "gtk",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!("QEMU exited with status: {s}"),
        Err(e) => {
            eprintln!("Could not launch QEMU: {e}");
            eprintln!("Ensure OVMF is installed and QEMU is in PATH.");
            print_virtualbox_instructions(UEFI_IMAGE);
        }
    }
}

fn print_virtualbox_instructions(image: &str) {
    println!();
    println!("─── VirtualBox quick-start ────────────────────────────────────");
    println!("  1. Convert:  qemu-img convert -f raw -O vmdk \\");
    println!("                 {image} argonOS.vmdk");
    println!("  2. New VM → Other Linux (64-bit) → Use existing disk → argonOS.vmdk");
    println!("  3. Settings → Serial Ports → Port 1: COM1 → File → argon-serial.log");
    println!("  4. Boot the VM – \"argonOS\" appears on screen.");
    println!("─────────────────────────────────────────────────────────────────");
}
