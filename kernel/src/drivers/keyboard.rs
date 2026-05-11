//! PS/2 keyboard driver with scancode-set-1 decoder and line discipline.
//!
//! The keyboard IRQ handler (already installed in the IDT) calls
//! `push_scancode(sc)` every time a key is pressed.  Higher-level code reads
//! ASCII characters with `pop_char()`.
//!
//! Line discipline (`readline`) buffers typed characters into a line buffer
//! and returns the completed line when Enter is pressed.  This is the minimal
//! functionality required to run an interactive shell.

use spin::Mutex;

// ---------------------------------------------------------------------------
// Ring buffer
// ---------------------------------------------------------------------------

const BUF_SIZE: usize = 256;

struct RingBuf {
    buf: [u8; BUF_SIZE],
    head: usize, // write index
    tail: usize, // read index
}

impl RingBuf {
    const fn new() -> Self {
        RingBuf {
            buf: [0u8; BUF_SIZE],
            head: 0,
            tail: 0,
        }
    }
    fn push(&mut self, b: u8) {
        let next = (self.head + 1) % BUF_SIZE;
        if next != self.tail {
            self.buf[self.head] = b;
            self.head = next;
        }
    }
    fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail {
            return None;
        }
        let b = self.buf[self.tail];
        self.tail = (self.tail + 1) % BUF_SIZE;
        Some(b)
    }
    fn is_empty(&self) -> bool {
        self.head == self.tail
    }
}

static SCANCODE_BUF: Mutex<RingBuf> = Mutex::new(RingBuf::new());

// ---------------------------------------------------------------------------
// Scancode → ASCII table (Set 1, US QWERTY, unshifted)
// ---------------------------------------------------------------------------

const SCANCODE_TABLE: [u8; 128] = [
    0, 0x1B, b'1', b'2', b'3', b'4', b'5', b'6', //  0– 7
    b'7', b'8', b'9', b'0', b'-', b'=', 0x08, b'\t', //  8–15
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', // 16–23
    b'o', b'p', b'[', b']', b'\n', 0, b'a', b's', // 24–31
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', // 32–39
    b'\'', b'`', 0, b'\\', b'z', b'x', b'c', b'v', // 40–47
    b'b', b'n', b'm', b',', b'.', b'/', 0, b'*', // 48–55
    0, b' ', 0, 0, 0, 0, 0, 0, // 56–63
    0, 0, 0, 0, 0, 0, 0, b'7', // 64–71
    b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1', // 72–79
    b'2', b'3', b'0', b'.', 0, 0, 0, 0, // 80–87
    0, 0, 0, 0, 0, 0, 0, 0, // 88–95
    0, 0, 0, 0, 0, 0, 0, 0, // 96–103
    0, 0, 0, 0, 0, 0, 0, 0, // 104–111
    0, 0, 0, 0, 0, 0, 0, 0, // 112–119
    0, 0, 0, 0, 0, 0, 0, 0, // 120–127
];

const SCANCODE_TABLE_SHIFTED: [u8; 128] = [
    0, 0x1B, b'!', b'@', b'#', b'$', b'%', b'^', //  0– 7
    b'&', b'*', b'(', b')', b'_', b'+', 0x08, b'\t', //  8–15
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', // 16–23
    b'O', b'P', b'{', b'}', b'\n', 0, b'A', b'S', // 24–31
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':', // 32–39
    b'"', b'~', 0, b'|', b'Z', b'X', b'C', b'V', // 40–47
    b'B', b'N', b'M', b'<', b'>', b'?', 0, b'*', // 48–55
    0, b' ', 0, 0, 0, 0, 0, 0, // 56–63
    0, 0, 0, 0, 0, 0, 0, b'7', // 64–71
    b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1', // 72–79
    b'2', b'3', b'0', b'.', 0, 0, 0, 0, // 80–87
    0, 0, 0, 0, 0, 0, 0, 0, // 88–95
    0, 0, 0, 0, 0, 0, 0, 0, // 96–103
    0, 0, 0, 0, 0, 0, 0, 0, // 104–111
    0, 0, 0, 0, 0, 0, 0, 0, // 112–119
    0, 0, 0, 0, 0, 0, 0, 0, // 120–127
];

// Shift-key state.
static SHIFT_HELD: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the keyboard driver (no hardware setup needed for PS/2).
pub fn init() {
    crate::serial_println!("keyboard: PS/2 driver ready");
}

/// Called from the keyboard IRQ handler with the raw scancode.
///
/// # Safety
/// Called from an interrupt context; must not block.
pub fn push_scancode(scancode: u8) {
    use core::sync::atomic::Ordering;

    // Track shift state (left shift = 0x2A, right shift = 0x36).
    match scancode {
        0x2A | 0x36 => {
            SHIFT_HELD.store(true, Ordering::Relaxed);
            return;
        }
        0xAA | 0xB6 => {
            SHIFT_HELD.store(false, Ordering::Relaxed);
            return;
        }
        _ => {}
    }

    // Ignore key-release events (bit 7 set) and extended scancodes.
    if scancode & 0x80 != 0 {
        return;
    }

    let table = if SHIFT_HELD.load(Ordering::Relaxed) {
        &SCANCODE_TABLE_SHIFTED
    } else {
        &SCANCODE_TABLE
    };

    let ascii = table[scancode as usize & 0x7F];
    if ascii != 0 {
        SCANCODE_BUF.lock().push(ascii);
    }
}

/// Pop one ASCII character from the keyboard ring buffer.
pub fn pop_char() -> Option<u8> {
    SCANCODE_BUF.lock().pop()
}

/// Block until a character is available, then return it.
pub fn read_char() -> u8 {
    loop {
        if let Some(c) = pop_char() {
            return c;
        }
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

/// Read a full line (terminated by Enter) into `buf`.
///
/// Returns the number of bytes written (excluding the NUL terminator).
/// The caller's buffer must be at least 1 byte.
pub fn readline(buf: &mut [u8]) -> usize {
    let mut pos = 0usize;
    loop {
        let c = read_char();
        match c {
            b'\n' | b'\r' => {
                if pos < buf.len() {
                    buf[pos] = 0;
                }
                return pos;
            }
            0x08 => {
                // Backspace.
                pos = pos.saturating_sub(1);
            }
            _ => {
                if pos + 1 < buf.len() {
                    buf[pos] = c;
                    pos += 1;
                }
            }
        }
    }
}

/// Check whether there are pending characters (non-blocking).
pub fn has_input() -> bool {
    !SCANCODE_BUF.lock().is_empty()
}
