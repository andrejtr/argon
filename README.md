# argonOS

A modern experimental x86-64 operating system kernel written in Rust.

```
                               ____   _____
                              / __ \ / ____|
   __ _ _ __ __ _  ___  _ __ | |  | | (___
  / _` | '__/ _` |/ _ \| '_ \| |  | |\___ \
 | (_| | | | (_| | (_) | | | | |__| |____) |
  \__,_|_|  \__, |\___/|_| |_|\____/|_____/
             __/ |
            |___/

            v0.1.0-alpha
```

---

## Features

### Security — on by default
| Feature | Detail |
|---|---|
| **SMEP** | Kernel cannot execute user-mode pages |
| **SMAP** | Kernel cannot read/write user-mode pages without explicit fence |
| **NXE / W^X** | No page is simultaneously writable and executable |
| **Stack canaries** | `-Z stack-protector=strong` — compiler-instrumented canary checks |
| **Double-fault IST** | Dedicated emergency stack — survives kernel stack overflow |
| **Rust** | Memory safety by construction — no buffer overflows, no use-after-free |

### Kernel subsystems
| Subsystem | Status |
|---|---|
| 64-bit long mode boot | ✅ |
| GDT + TSS | ✅ |
| IDT (CPU exceptions + IRQs) | ✅ |
| 8259 PIC (IRQ remapping) | ✅ |
| PIT timer @ 100 Hz | ✅ |
| Serial output (COM1) | ✅ |
| Heap allocator (1 MiB) | ✅ |
| Framebuffer pixel renderer | ✅ |
| VGA text-mode fallback | ✅ |
| Round-robin scheduler | ✅ |
| Syscall gate (INT 0x80) | ✅ |
| RamFS (in-memory filesystem) | ✅ |
| User-mode processes | 🔜 |
| Networking | 🔜 |

---

## Requirements

| Tool | Version |
|---|---|
| Rust | nightly (pinned via `rust-toolchain.toml`) |
| Target | `x86_64-unknown-none` (installed automatically) |
| QEMU | `qemu-system-x86_64` — for local testing |
| VirtualBox | for `.vmdk` images (optional) |

Install Rust via [rustup](https://rustup.rs):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

The correct nightly toolchain and components are installed automatically by `rust-toolchain.toml`.

---

## Building

```bash
# Clone
git clone https://github.com/andrejtr/argon.git
cd argon

# Build the kernel ELF only (fast — no disk image)
cargo build-kernel

# Build release kernel
cargo build-kernel --release

# Build bootable BIOS + UEFI disk images
cargo build-images
```

Disk images are written to `target/disk-builder-out/`:
- `bios.img` — raw BIOS-bootable disk image
- `uefi.img` — raw UEFI-bootable disk image

---

## Running

### QEMU (fastest)

```bash
cargo run-images            # BIOS (default)
cargo run-images -- --uefi  # UEFI
```

Serial output goes to your terminal. You should see:

```
argonOS booting...
arch: GDT/TSS/IDT/PIC/PIT loaded, interrupts enabled
memory: physical offset 0xffff800000000000
memory: OK  (heap 1 MiB)
display: framebuffer 1024x768 OK
ramfs: /etc/os-release = "NAME=argonOS\nVERSION=1.0.0.0\n"
scheduler: running
argonOS ready.
task-A: tick 0
task-B: tick 0
```

### VirtualBox

1. Build the VMDK:
   ```bash
   qemu-img convert -f raw -O vmdk target/disk-builder-out/bios.img argonOS.vmdk
   ```
2. New VM → **Other/Unknown (64-bit)** → use existing virtual hard disk → select `argonOS.vmdk`
3. Settings → Serial Ports → Port 1 → enable → pipe to a file to capture serial output
4. Boot

---

## Project layout

```
argon/
├── kernel/                  # Bare-metal kernel crate (no_std)
│   └── src/
│       ├── main.rs          # Entry point — kernel_main()
│       ├── arch/
│       │   └── x86_64/
│       │       ├── gdt.rs   # Global Descriptor Table + TSS
│       │       ├── idt.rs   # Interrupt Descriptor Table + handlers
│       │       ├── pic.rs   # 8259 PIC driver
│       │       ├── pit.rs   # PIT timer @ 100 Hz
│       │       └── security.rs  # SMEP / SMAP / NXE hardening
│       ├── display/
│       │   ├── framebuffer.rs   # Pixel renderer + ASCII-art splash
│       │   └── vga.rs           # VGA text-mode fallback
│       ├── future/
│       │   ├── scheduler.rs  # Round-robin preemptive scheduler
│       │   ├── ramfs.rs      # In-memory filesystem
│       │   ├── vfs.rs        # FileSystem trait
│       │   ├── process.rs    # Process control block
│       │   └── syscall.rs    # INT 0x80 dispatch
│       ├── memory/
│       │   ├── heap.rs       # Global heap allocator (1 MiB)
│       │   ├── paging.rs     # OffsetPageTable
│       │   └── allocator.rs  # Physical frame allocator
│       ├── serial.rs         # UART COM1 + serial_println! macro
│       └── panic.rs          # #[panic_handler]
├── disk-builder/            # Host tool — produces bootable images
│   ├── build.rs             # Invokes bootloader crate to create images
│   └── src/main.rs          # CLI: QEMU launch or VirtualBox instructions
├── .github/workflows/
│   ├── ci.yml               # Build, Clippy, rustfmt, cargo-audit
│   └── release.yml          # Tag → bios.img + uefi.img + bios.vmdk release
├── rust-toolchain.toml      # Pins nightly + components
└── Cargo.toml               # Workspace root
```

---

## CI / CD

Every push and pull request runs:

| Job | What it checks |
|---|---|
| **Build & Lint** | `cargo build` debug + release, Clippy `-D warnings`, disk images |
| **Rustfmt** | `cargo fmt --check` |
| **Security Audit** | `cargo audit` — CVE scan of all dependencies |

Tagging a release (`v*.*.*`) triggers a GitHub Release with:
- `argonOS-{TAG}-bios.img`
- `argonOS-{TAG}-uefi.img`
- `argonOS-{TAG}-bios.vmdk` (VirtualBox-ready)
- SHA-256 checksums

---

## License

MIT — see [LICENSE](LICENSE) if present, otherwise all rights reserved.
