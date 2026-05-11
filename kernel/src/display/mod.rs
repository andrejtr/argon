pub mod framebuffer;
pub mod vga;
use crate::{serial_println, vga_println};

use bootloader_api::BootInfo;

/// Initialise the display subsystem and render the boot splash.
///
/// Tries the pixel framebuffer first (available when the bootloader configures
/// one).  Falls back to VGA text mode (always available on x86 PCs).
pub fn init(boot_info: &'static mut BootInfo) {
    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        let buf  = fb.buffer_mut();
        let mut renderer = framebuffer::FramebufferRenderer::new(buf, info);
        renderer.draw_splash();
        serial_println!("display: framebuffer {}×{} OK", info.width, info.height);
    } else {
        // VGA text-mode fallback.
        vga::init();
        vga_println!("  ___ _ _ _ _  ___  ___ ");
        vga_println!(" | _ || | | || _ \\/ _ \\ ");
        vga_println!(" | _/| | | ||  _/| (_) |");
        vga_println!(" |_|  \\___|_||_|   \\___/ ");
        vga_println!("");
        vga_println!("  argonOS  |  Rust x86_64  |  Secure");
        serial_println!("display: VGA text mode OK");
    }
}
