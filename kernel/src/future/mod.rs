#![allow(dead_code)]
//! Unix-foundation scaffolding.
//!
//! These modules are not yet wired into the running kernel; they define
//! the typed interfaces and data structures that the next development
//! phases will build on.

pub mod elf;
pub mod process;
pub mod ramfs;
pub mod scheduler;
pub mod syscall;
pub mod vfs;
