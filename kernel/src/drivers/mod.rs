//! Kernel driver subsystem.

pub mod ahci;
pub mod keyboard;
pub mod nvme;
pub mod pci;
// Block-device abstraction is in crate::block (not under drivers).
