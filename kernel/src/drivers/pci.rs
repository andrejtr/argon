//! PCI configuration space scanner (legacy CF8/CFC port I/O mechanism).
//!
//! Iterates all 256 buses × 32 devices × 8 functions and calls a closure for
//! each present device.

use x86_64::instructions::port::{Port, PortReadOnly};

/// Compact description of one PCI device/function.
#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
}

impl PciDevice {
    /// Read a 32-bit doubleword from PCI configuration space.
    pub fn cfg_read32(&self, offset: u8) -> u32 {
        cfg_read32(self.bus, self.device, self.function, offset)
    }

    /// Read a 16-bit word from PCI configuration space.
    pub fn cfg_read16(&self, offset: u8) -> u16 {
        (self.cfg_read32(offset & !3) >> ((offset & 2) * 8)) as u16
    }

    /// Read the Base Address Register at BAR index 0–5.
    pub fn bar(&self, index: u8) -> u64 {
        let offset = 0x10 + index * 4;
        let lo = self.cfg_read32(offset) as u64;
        if lo & 0x4 != 0 {
            // 64-bit BAR: upper 32 bits in next register.
            let hi = self.cfg_read32(offset + 4) as u64;
            ((hi << 32) | lo) & !0xF
        } else {
            lo & !0xF
        }
    }

    /// Enable Bus-Master DMA and Memory-Mapped I/O in the PCI Command register.
    pub fn enable_bus_master(&self) {
        let cmd = self.cfg_read16(0x04);
        cfg_write16(self.bus, self.device, self.function, 0x04, cmd | 0x0006);
    }
}

// ---------------------------------------------------------------------------
// Raw port I/O helpers
// ---------------------------------------------------------------------------

/// Compute the 32-bit PCI address word.
fn pci_addr(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    0x8000_0000
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC)
}

/// Read 32 bits from PCI config space.
pub fn cfg_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    unsafe {
        Port::<u32>::new(0xCF8).write(pci_addr(bus, dev, func, offset));
        PortReadOnly::<u32>::new(0xCFC).read()
    }
}

/// Write 16 bits to PCI config space.
pub fn cfg_write16(bus: u8, dev: u8, func: u8, offset: u8, val: u16) {
    let shift = (offset & 2) * 8;
    let mut dword = cfg_read32(bus, dev, func, offset & !3);
    dword &= !(0xFFFF << shift);
    dword |= (val as u32) << shift;
    unsafe {
        Port::<u32>::new(0xCF8).write(pci_addr(bus, dev, func, offset & !3));
        Port::<u32>::new(0xCFC).write(dword);
    }
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// Scan all PCI buses and call `f` for every present device.
pub fn scan<F: FnMut(PciDevice)>(mut f: F) {
    for bus in 0u8..=255 {
        for dev in 0u8..32 {
            let raw = cfg_read32(bus, dev, 0, 0x00);
            if raw == 0xFFFF_FFFF {
                continue; // no device
            }
            let vendor_id = (raw & 0xFFFF) as u16;
            let device_id = (raw >> 16) as u16;

            let class_raw = cfg_read32(bus, dev, 0, 0x08);
            let class = (class_raw >> 24) as u8;
            let subclass = (class_raw >> 16) as u8;
            let prog_if = (class_raw >> 8) as u8;

            let hdr_raw = cfg_read32(bus, dev, 0, 0x0C);
            let header_type = (hdr_raw >> 16) as u8;
            let max_func = if header_type & 0x80 != 0 { 8u8 } else { 1u8 };

            for func in 0..max_func {
                let fraw = cfg_read32(bus, dev, func, 0x00);
                if fraw == 0xFFFF_FFFF {
                    continue;
                }
                let fclass = cfg_read32(bus, dev, func, 0x08);
                let fhdr = cfg_read32(bus, dev, func, 0x0C);
                f(PciDevice {
                    bus,
                    device: dev,
                    function: func,
                    vendor_id: (fraw & 0xFFFF) as u16,
                    device_id: (fraw >> 16) as u16,
                    class: (fclass >> 24) as u8,
                    subclass: (fclass >> 16) as u8,
                    prog_if: (fclass >> 8) as u8,
                    header_type: (fhdr >> 16) as u8,
                });
            }

            // Avoid reporting the same single-function device twice.
            let _ = (vendor_id, device_id, class, subclass, prog_if, header_type);
        }
    }
}

/// Find the first PCI device matching `(class, subclass)`.
pub fn find_device(class: u8, subclass: u8) -> Option<PciDevice> {
    let mut result = None;
    scan(|dev| {
        if result.is_none() && dev.class == class && dev.subclass == subclass {
            result = Some(dev);
        }
    });
    result
}
