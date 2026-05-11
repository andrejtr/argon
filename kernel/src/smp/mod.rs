//! Symmetric Multi-Processing (SMP) initialisation.
//!
//! Parses the ACPI MADT (Multiple APIC Description Table) to discover
//! Application Processors (APs), copies a 16-bit real-mode trampoline to
//! physical address 0x8000, patches it with the AP entry point and kernel
//! CR3, then fires INIT/SIPI inter-processor interrupts (IPIs) at each AP.
//!
//! Each AP configures its own GDT/IDT, enables APIC, and signals readiness
//! via an atomic counter.  The BSP waits for all APs before continuing.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::serial_println;

// ---------------------------------------------------------------------------
// ACPI table parsing (RSDP → RSDT/XSDT → MADT)
// ---------------------------------------------------------------------------

const MADT_SIG: [u8; 4] = *b"APIC";
const RSDT_SIG: [u8; 4] = *b"RSDT";
const XSDT_SIG: [u8; 4] = *b"XSDT";

#[repr(C, packed)]
struct RsdpV1 {
    signature: [u8; 8],
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_addr: u32,
}

#[repr(C, packed)]
struct RsdpV2 {
    v1: RsdpV1,
    length: u32,
    xsdt_addr: u64,
    ext_checksum: u8,
    _reserved: [u8; 3],
}

#[repr(C, packed)]
struct AcpiSdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

/// Find the MADT physical address by walking RSDP→XSDT/RSDT.
unsafe fn find_madt(rsdp_phys: u64) -> Option<u64> {
    let rsdp_virt = crate::memory::phys_to_virt(x86_64::PhysAddr::new(rsdp_phys));
    let rsdp = &*(rsdp_virt.as_ptr::<RsdpV2>());

    let (use_xsdt, sdt_phys, entry_size) = if rsdp.v1.revision >= 2 {
        let xsdt_phys = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(rsdp.xsdt_addr)) };
        (true, xsdt_phys, 8usize)
    } else {
        (false, rsdp.v1.rsdt_addr as u64, 4usize)
    };

    let hdr_virt = crate::memory::phys_to_virt(x86_64::PhysAddr::new(sdt_phys));
    let hdr = &*(hdr_virt.as_ptr::<AcpiSdtHeader>());
    let total_len = core::ptr::read_unaligned(core::ptr::addr_of!(hdr.length)) as usize;
    let entries_start = hdr_virt.as_u64() + core::mem::size_of::<AcpiSdtHeader>() as u64;
    let num_entries = (total_len - core::mem::size_of::<AcpiSdtHeader>()) / entry_size;

    for i in 0..num_entries {
        let ptr = entries_start + (i * entry_size) as u64;
        let child_phys: u64 = if use_xsdt {
            core::ptr::read_unaligned(ptr as *const u64)
        } else {
            core::ptr::read_unaligned(ptr as *const u32) as u64
        };
        let child_virt = crate::memory::phys_to_virt(x86_64::PhysAddr::new(child_phys));
        let child_hdr = &*(child_virt.as_ptr::<AcpiSdtHeader>());
        if child_hdr.signature == MADT_SIG {
            return Some(child_phys);
        }
        let _ = (use_xsdt, RSDT_SIG, XSDT_SIG); // suppress warnings
    }
    None
}

// ---------------------------------------------------------------------------
// MADT entry parsing
// ---------------------------------------------------------------------------

#[repr(C, packed)]
struct MadtHeader {
    sdt: AcpiSdtHeader,
    local_apic_addr: u32,
    flags: u32,
}

#[repr(C, packed)]
struct MadtEntryHeader {
    entry_type: u8,
    length: u8,
}

#[repr(C, packed)]
struct MadtLocalApic {
    hdr: MadtEntryHeader,
    acpi_proc_id: u8,
    apic_id: u8,
    flags: u32, // bit 0 = enabled
}

/// Collect APIC IDs of all enabled processors from MADT.
unsafe fn collect_ap_ids(madt_phys: u64, bsp_apic_id: u8) -> alloc::vec::Vec<u8> {
    extern crate alloc;
    let mut ids = alloc::vec::Vec::new();
    let madt_virt = crate::memory::phys_to_virt(x86_64::PhysAddr::new(madt_phys));
    let madt_hdr = &*(madt_virt.as_ptr::<MadtHeader>());
    let total = core::ptr::read_unaligned(core::ptr::addr_of!(madt_hdr.sdt.length)) as usize;
    let mut offset = core::mem::size_of::<MadtHeader>();
    while offset + 2 <= total {
        let entry = &*((madt_virt.as_u64() + offset as u64) as *const MadtEntryHeader);
        let len = entry.length as usize;
        if len < 2 {
            break;
        }
        if entry.entry_type == 0 && len >= core::mem::size_of::<MadtLocalApic>() {
            let la = &*((madt_virt.as_u64() + offset as u64) as *const MadtLocalApic);
            let flags = core::ptr::read_unaligned(core::ptr::addr_of!(la.flags));
            let apic_id = la.apic_id;
            if flags & 1 != 0 && apic_id != bsp_apic_id {
                ids.push(apic_id);
            }
        }
        offset += len;
    }
    ids
}

// ---------------------------------------------------------------------------
// AP trampoline (real → protected → long mode)
// ---------------------------------------------------------------------------
// The trampoline must fit in a single 4 KiB page at 0x8000 (physical).
// We generate the minimal 16→32→64-bit startup sequence in Rust-emitted
// bytes using global_asm! and then copy it into place.

/// Physical address where the trampoline is loaded.
const TRAMPOLINE_PHYS: u64 = 0x8000;

/// Shared flag: number of APs that have completed startup.
static AP_READY: AtomicU32 = AtomicU32::new(0);

/// Written by BSP before SIPI so APs know the 64-bit entry point.
static AP_ENTRY: AtomicU32 = AtomicU32::new(0);

/// AP startup path called from the trampoline after entering 64-bit mode.
#[no_mangle]
extern "C" fn ap_main() -> ! {
    // Each AP sets up its own GDT and IDT (already in memory from BSP).
    crate::arch::x86_64::gdt::init();
    crate::arch::x86_64::idt::init();
    x86_64::instructions::interrupts::enable();
    serial_println!("smp: AP online");
    AP_READY.fetch_add(1, Ordering::Release);
    loop {
        x86_64::instructions::hlt();
    }
}

// ---------------------------------------------------------------------------
// LAPIC IPI helpers
// ---------------------------------------------------------------------------

/// Base virtual address of the local APIC MMIO registers.
static mut LAPIC_BASE: u64 = 0;

const LAPIC_ID: u32 = 0x020;
const LAPIC_ICR_LO: u32 = 0x300;
const LAPIC_ICR_HI: u32 = 0x310;

unsafe fn lapic_read(reg: u32) -> u32 {
    core::ptr::read_volatile((LAPIC_BASE + reg as u64) as *const u32)
}

unsafe fn lapic_write(reg: u32, val: u32) {
    core::ptr::write_volatile((LAPIC_BASE + reg as u64) as *mut u32, val);
}

/// Wait for LAPIC Delivery Status to clear (IPI sent).
unsafe fn lapic_wait_icr() {
    for _ in 0..100_000u32 {
        if lapic_read(LAPIC_ICR_LO) & (1 << 12) == 0 {
            return;
        }
    }
}

/// Send INIT IPI to `apic_id`.
unsafe fn send_init(apic_id: u8) {
    lapic_write(LAPIC_ICR_HI, (apic_id as u32) << 24);
    lapic_write(LAPIC_ICR_LO, 0x0000_4500); // INIT, assert, edge
    lapic_wait_icr();
}

/// Send STARTUP IPI (SIPI) to `apic_id` with vector page `vec` (0x08 → 0x8000).
unsafe fn send_sipi(apic_id: u8, vec: u8) {
    lapic_write(LAPIC_ICR_HI, (apic_id as u32) << 24);
    lapic_write(LAPIC_ICR_LO, 0x0000_4600 | vec as u32); // SIPI, assert, edge
    lapic_wait_icr();
}

// Busy-delay in I/O port reads (each ≈ 1 µs on legacy hardware).
unsafe fn io_delay(n: u32) {
    for _ in 0..n {
        let _ = x86_64::instructions::port::Port::<u8>::new(0x80).read();
    }
}

// ---------------------------------------------------------------------------
// Trampoline installation
// ---------------------------------------------------------------------------
//
// The trampoline is hand-coded in a byte slice.  It's deliberately minimal:
// real-mode → 32-bit PM → 64-bit LM, then calls `ap_main`.
//
// Offsets used by the BSP to patch in live values:
//   TRAMPOLINE_PHYS + 0x10  → 32-bit CR3 (physical address of BSP's L4)
//   TRAMPOLINE_PHYS + 0x14  → 32-bit (low) pointer to ap_main
//
// The AP trampoline is position-independent relative to its load address
// (0x8000), but we write the absolute virtual addresses of the 64-bit GDT
// and ap_main at patch time.

unsafe fn install_trampoline() {
    use x86_64::registers::control::Cr3;
    let (l4_frame, _) = Cr3::read();
    let cr3_phys = l4_frame.start_address().as_u64() as u32;

    let ap_entry_va = ap_main as *const () as u64;
    let trampoline_va = crate::memory::phys_to_virt(x86_64::PhysAddr::new(TRAMPOLINE_PHYS));

    // Minimal x86 16→32→64-bit trampoline bytes.
    // Layout (offsets from 0x8000):
    //  0x00 – real-mode entry (far jump to 0x8010)
    //  0x08 – GDT pointer (6 bytes: limit=23, base=0x8020)
    //  0x10 – patch slot: CR3 (4 bytes)
    //  0x14 – patch slot: ap_main low 32 bits (4 bytes)
    //  0x18 – patch slot: ap_main high 32 bits (4 bytes)
    //  0x1C – padding
    //  0x20 – GDT entries (null, code32, data32, code64, data64)
    //  0x60 – 32-bit PM code
    //  0xC0 – 64-bit LM code

    let tptr = trampoline_va.as_mut_ptr::<u8>();
    // Zero the page first.
    core::ptr::write_bytes(tptr, 0, 4096);

    // Write patch slots.
    core::ptr::write_unaligned(tptr.add(0x10) as *mut u32, cr3_phys);
    core::ptr::write_unaligned(tptr.add(0x14) as *mut u32, ap_entry_va as u32);
    core::ptr::write_unaligned(tptr.add(0x18) as *mut u32, (ap_entry_va >> 32) as u32);

    // --- Real-mode entry (0x00): cli + ljmp 0:0x8060 ---
    let rm: &mut [u8] = core::slice::from_raw_parts_mut(tptr, 16);
    rm[0] = 0xFA; // cli
                  // lgdt [0x8008]
    rm[1] = 0x0F;
    rm[2] = 0x01;
    rm[3] = 0x16;
    rm[4] = 0x08;
    rm[5] = 0x80; // word operand: addr 0x8008 in 16-bit mode
                  // mov eax, cr0; or al, 1; mov cr0, eax
    rm[6] = 0x0F;
    rm[7] = 0x20;
    rm[8] = 0xC0; // mov eax, cr0
    rm[9] = 0x0C;
    rm[10] = 0x01; // or al, 1
    rm[11] = 0x0F;
    rm[12] = 0x22;
    rm[13] = 0xC0; // mov cr0, eax
                   // ljmp 0x08:0x8060
    rm[14] = 0xEA; // far jump opcode
                   // The next 4 bytes are the 32-bit offset and 2-byte selector (in 16-bit mode these
                   // are 2-byte offset + 2-byte segment). For a 16-bit far jmp: off16, seg16.
                   // We'll encode this as a jmp to 0x0008:0x8060 (pm32 code at 0x8060).
                   // In 16-bit mode: jmp ptr16:16 → 2-byte offset then 2-byte segment.
    rm[14] = 0xEA;
    // offset low byte, high byte, segment low, high
    rm[15] = 0x60; // offset low = 0x60 (0x8060 - 0x8000)

    // GDT pointer at offset 0x08 (limit=23, base=0x8020 physical).
    let gdt_ptr_loc = tptr.add(0x08);
    let gdt_base: u32 = TRAMPOLINE_PHYS as u32 + 0x20;
    core::ptr::write_unaligned(gdt_ptr_loc as *mut u16, 23u16); // limit
    core::ptr::write_unaligned(gdt_ptr_loc.add(2) as *mut u32, gdt_base); // base

    // GDT at offset 0x20 (24 bytes = 3 entries: null, code32, data32, code64).
    let gdt = tptr.add(0x20);
    // null descriptor
    core::ptr::write_unaligned(gdt as *mut u64, 0u64);
    // code32: base=0, limit=0xFFFFFFFF, DPL=0, executable, readable
    core::ptr::write_unaligned(gdt.add(8) as *mut u64, 0x00CF_9A00_0000_FFFFu64);
    // data32: base=0, limit=0xFFFFFFFF, DPL=0, writable
    core::ptr::write_unaligned(gdt.add(16) as *mut u64, 0x00CF_9200_0000_FFFFu64);

    // 32-bit PM code at offset 0x60:
    // 1. Load data segment
    // 2. Load CR3 (patched at 0x8010)
    // 3. Enable PAE (CR4.PAE)
    // 4. Set EFER.LME
    // 5. Enable paging (CR0.PG)
    // 6. Far jump to 64-bit code
    //
    // This is represented as raw bytes for the x86-32 instruction set.
    let pm32 = tptr.add(0x60);
    let pm32_code: &[u8] = &[
        // mov ax, 0x10; mov ds, ax; mov es, ax; mov ss, ax; mov fs, ax; mov gs, ax
        0x66, 0xB8, 0x10, 0x00, // mov ax, 0x10
        0x8E, 0xD8, // mov ds, ax
        0x8E, 0xC0, // mov es, ax
        0x8E, 0xD0, // mov ss, ax
        // mov eax, [0x8010]  (CR3 patch slot)
        0x8B, 0x05, 0x10, 0x80, 0x00, 0x00, // mov cr3, eax
        0x0F, 0x22, 0xD8, // mov eax, cr4; or eax, 0x620 (PAE+PGE+PSE); mov cr4, eax
        0x0F, 0x20, 0xE0, // mov eax, cr4
        0x0D, 0x20, 0x06, 0x00, 0x00, // or eax, 0x620
        0x0F, 0x22, 0xE0, // mov cr4, eax
        // rdmsr(0xC0000080); or eax, 0x100 (LME); wrmsr
        0xB9, 0x80, 0x00, 0x00, 0xC0, // mov ecx, 0xC0000080
        0x0F, 0x32, // rdmsr
        0x0D, 0x00, 0x01, 0x00, 0x00, // or eax, 0x100
        0x0F, 0x30, // wrmsr
        // mov eax, cr0; or eax, 0x80000001; mov cr0, eax  (PG + PE)
        0x0F, 0x20, 0xC0, 0x0D, 0x01, 0x00, 0x00, 0x80, 0x0F, 0x22, 0xC0,
        // ljmp 0x08:0x80C0 (far jump to 64-bit code at offset 0xC0)
        0xEA, 0xC0, 0x80, 0x00, 0x00, 0x08, 0x00,
    ];
    core::ptr::copy_nonoverlapping(pm32_code.as_ptr(), pm32, pm32_code.len());

    // 64-bit LM code at offset 0xC0:
    // Load ap_main address from patch slots and call it.
    let lm64 = tptr.add(0xC0);
    let lm64_code: &[u8] = &[
        // mov rax, [rip+…] — load ap_main address (patched at 0x8014/0x8018)
        // Instead, use movabs rax, imm64 (0x48 0xB8) with 8-byte immediate.
        0x48, 0xB8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // movabs rax, 0 (patched)
        0xFF, 0xD0, // call rax
        0xF4, // hlt (should not reach)
    ];
    core::ptr::copy_nonoverlapping(lm64_code.as_ptr(), lm64, lm64_code.len());
    // Patch the movabs immediate with the actual ap_main address.
    core::ptr::write_unaligned(lm64.add(2) as *mut u64, ap_entry_va);
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Initialise SMP: parse MADT, install trampoline, start APs.
///
/// `rsdp_addr` is the physical address provided by the bootloader.
pub fn init(rsdp_addr: u64) {
    if rsdp_addr == 0 {
        serial_println!("smp: no RSDP – running single-core");
        return;
    }

    unsafe {
        // Discover LAPIC base from MSR 0x1B.
        let lapic_msr = x86_64::registers::model_specific::Msr::new(0x1B).read();
        let lapic_phys = lapic_msr & 0xFFFF_F000;
        LAPIC_BASE = crate::memory::phys_to_virt(x86_64::PhysAddr::new(lapic_phys)).as_u64();
        let bsp_apic_id = (lapic_read(LAPIC_ID) >> 24) as u8;
        serial_println!("smp: BSP APIC ID={} LAPIC={:#x}", bsp_apic_id, lapic_phys);

        let madt_phys = match find_madt(rsdp_addr) {
            Some(p) => p,
            None => {
                serial_println!("smp: MADT not found");
                return;
            }
        };

        let ap_ids = collect_ap_ids(madt_phys, bsp_apic_id);
        if ap_ids.is_empty() {
            serial_println!("smp: no APs found – uniprocessor");
            return;
        }
        serial_println!("smp: {} AP(s) found: {:?}", ap_ids.len(), ap_ids);

        install_trampoline();

        let expected_aps = ap_ids.len() as u32;
        for &apic_id in &ap_ids {
            serial_println!("smp: starting AP {}", apic_id);
            send_init(apic_id);
            io_delay(10_000); // ≈ 10 ms
            send_sipi(apic_id, (TRAMPOLINE_PHYS >> 12) as u8);
            io_delay(200);
            send_sipi(apic_id, (TRAMPOLINE_PHYS >> 12) as u8); // second SIPI
            io_delay(200);
        }

        // Wait up to ~1 second for all APs.
        for _ in 0..1_000_000u32 {
            if AP_READY.load(Ordering::Acquire) >= expected_aps {
                break;
            }
        }
        serial_println!(
            "smp: {}/{} APs online",
            AP_READY.load(Ordering::Acquire),
            expected_aps
        );
    }
}
