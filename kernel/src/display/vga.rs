/// VGA text-mode driver.
///
/// The VGA text buffer lives at physical address `0xB8000`.  Each cell is two
/// bytes: the ASCII character and a colour attribute byte.  The visible area
/// is 80 columns × 25 rows.
///
/// All reads/writes go through `core::ptr::read_volatile` / `write_volatile`
/// so the compiler never optimises them away.
use core::fmt;
use core::ptr;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Colour definitions
// ---------------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    const fn new(fg: Color, bg: Color) -> Self {
        ColorCode((bg as u8) << 4 | (fg as u8))
    }
}

// ---------------------------------------------------------------------------
// Buffer layout
// ---------------------------------------------------------------------------

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;
const VGA_BUFFER: *mut u16 = 0xB8000 as *mut u16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii: u8,
    color: ColorCode,
}

impl ScreenChar {
    fn as_u16(self) -> u16 {
        u16::from(self.ascii) | (u16::from(self.color.0) << 8)
    }
    fn from_u16(raw: u16) -> Self {
        Self {
            ascii: raw as u8,
            color: ColorCode(((raw >> 8) & 0xFF) as u8),
        }
    }
}

fn vga_write(row: usize, col: usize, sc: ScreenChar) {
    let offset = row * BUFFER_WIDTH + col;
    // SAFETY: VGA_BUFFER points to the memory-mapped VGA text buffer.
    unsafe { ptr::write_volatile(VGA_BUFFER.add(offset), sc.as_u16()) }
}

fn vga_read(row: usize, col: usize) -> ScreenChar {
    let offset = row * BUFFER_WIDTH + col;
    // SAFETY: same as vga_write.
    ScreenChar::from_u16(unsafe { ptr::read_volatile(VGA_BUFFER.add(offset)) })
}

pub struct Writer {
    column: usize,
    row: usize,
    color: ColorCode,
}

impl Writer {
    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.column >= BUFFER_WIDTH {
                    self.new_line();
                }
                vga_write(
                    self.row,
                    self.column,
                    ScreenChar {
                        ascii: byte,
                        color: self.color,
                    },
                );
                self.column += 1;
            }
        }
    }

    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            match byte {
                0x20..=0x7e | b'\n' => self.write_byte(byte),
                // Replace non-printable / non-ASCII with a placeholder.
                _ => self.write_byte(0xfe),
            }
        }
    }

    fn new_line(&mut self) {
        if self.row < BUFFER_HEIGHT - 1 {
            self.row += 1;
        } else {
            self.scroll();
        }
        self.column = 0;
    }

    fn scroll(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let ch = vga_read(row, col);
                vga_write(row - 1, col, ch);
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column = 0;
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii: b' ',
            color: self.color,
        };
        for col in 0..BUFFER_WIDTH {
            vga_write(row, col, blank);
        }
    }

    pub fn clear_screen(&mut self) {
        for row in 0..BUFFER_HEIGHT {
            self.clear_row(row);
        }
        self.row = 0;
        self.column = 0;
    }

    pub fn set_color(&mut self, fg: Color, bg: Color) {
        self.color = ColorCode::new(fg, bg);
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

pub static WRITER: Mutex<Option<Writer>> = Mutex::new(None);

/// Initialise the VGA writer.
///
/// # Safety
/// Must only be called once.
pub fn init() {
    let mut writer = Writer {
        column: 0,
        row: 0,
        color: ColorCode::new(Color::LightCyan, Color::Black),
    };
    writer.clear_screen();
    *WRITER.lock() = Some(writer);
}

// ---------------------------------------------------------------------------
// Public print macros
// ---------------------------------------------------------------------------

#[macro_export]
macro_rules! vga_print {
    ($($arg:tt)*) => {
        $crate::display::vga::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! vga_println {
    ()            => ($crate::vga_print!("\n"));
    ($($arg:tt)*) => ($crate::vga_print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use fmt::Write;
    if let Some(ref mut w) = *WRITER.lock() {
        w.write_fmt(args).ok();
    }
}
