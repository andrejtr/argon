use spin::Mutex;
use uart_16550::SerialPort;

/// COM1 base I/O port – present on every x86 PC and emulated by QEMU/VirtualBox.
const COM1: u16 = 0x3F8;

static SERIAL: Mutex<Option<SerialPort>> = Mutex::new(None);

/// Initialise COM1 at 115 200 baud (hardware default after SerialPort::init).
pub fn init() {
    let mut port = unsafe { SerialPort::new(COM1) };
    port.init();
    *SERIAL.lock() = Some(port);
}

/// Write a single byte to the serial port.  Safe to call after `init()`.
pub fn write_byte(byte: u8) {
    if let Some(ref mut port) = *SERIAL.lock() {
        port.send(byte);
    }
}

/// Write a UTF-8 string to the serial port.
pub fn write_str(s: &str) {
    for byte in s.bytes() {
        match byte {
            // Replace bare LF with CR+LF so terminals render correctly.
            b'\n' => {
                write_byte(b'\r');
                write_byte(b'\n');
            }
            b => write_byte(b),
        }
    }
}

// ---------------------------------------------------------------------------
// fmt::Write implementation so the serial_print! macros work.
// ---------------------------------------------------------------------------

use core::fmt;

pub struct SerialWriter;

impl fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        write_str(s);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public macros
// ---------------------------------------------------------------------------

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! serial_println {
    ()           => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use fmt::Write;
    SerialWriter.write_fmt(args).ok();
}
