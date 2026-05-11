/// Hardware security hardening.
///
/// This module is the first thing called during `arch::init()`.  It enables
/// the CPU security features that the kernel relies on:
///
/// * **SMEP** – Supervisor Mode Execution Prevention: prevents the kernel from
///   executing pages mapped as user-mode (ring 3) code.  This blocks trivial
///   ret2user / privilege-escalation exploits.
///
/// * **SMAP** – Supervisor Mode Access Prevention: prevents the kernel from
///   *reading or writing* user-mode pages without an explicit `STAC`/`CLAC`
///   pair.  Eliminates a whole class of confused-deputy bugs.
///
/// * **NXE (No-Execute Enable)** – enables the NX/XD bit in page table
///   entries, which is required for W^X enforcement.
///
/// None of these are enabled by the bootloader by default.
use crate::serial_println;
use x86_64::registers::control::{Cr4, Cr4Flags};
use x86_64::registers::model_specific::{Efer, EferFlags};

// ---------------------------------------------------------------------------
// Stack-protector support symbols
//
// The `-Z stack-protector=strong` flag instruments every function with a
// canary check.  In a hosted environment the C runtime provides these.  In a
// bare-metal kernel we supply our own.
//
// `__stack_chk_guard` – the canary value; ideally seeded from a hardware
//   PRNG (RDRAND) but a fixed sentinel is acceptable for an MVP.
// `__stack_chk_fail` – called on canary mismatch; we halt the kernel.
// ---------------------------------------------------------------------------

/// Stack canary value.  Placed in `.data` so it is not in a read-only section
/// where the attacker could more easily predict its location.
#[no_mangle]
pub static mut __stack_chk_guard: u64 = 0xDEAD_BEEF_CAFE_BABE;

/// Called by the compiler when a stack-smashing attack is detected.
#[no_mangle]
pub extern "C" fn __stack_chk_fail() -> ! {
    // We cannot trust the stack at this point; jump straight to a safe halt.
    loop {
        x86_64::instructions::interrupts::disable();
        x86_64::instructions::hlt();
    }
}

/// Enable SMEP, SMAP, and NXE.
///
/// # Safety
/// Must be called in a controlled boot environment before any user-mode or
/// potentially-hostile code runs.  Enabling SMAP requires that all kernel
/// memory accesses to user-mode addresses use `STAC`/`CLAC` wrappers;
/// since argonOS currently has no user-mode, this is unconditionally safe.
pub fn harden() {
    // --- CR4: SMEP + SMAP ---
    // SAFETY: We own the CPU at this point in boot.  No user-mode pages exist
    // yet so SMAP cannot incorrectly fault any kernel access.
    unsafe {
        let mut cr4 = Cr4::read();
        cr4.insert(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION);
        cr4.insert(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION);
        Cr4::write(cr4);
    }

    // --- EFER.NXE: No-Execute Enable ---
    // SAFETY: Required before any page table entry with the NX bit is loaded.
    // The bootloader may already set this; inserting the flag is idempotent.
    unsafe {
        Efer::update(|flags| {
            flags.insert(EferFlags::NO_EXECUTE_ENABLE);
        });
    }

    serial_println!("security: SMEP ON  SMAP ON  NXE ON");
}
