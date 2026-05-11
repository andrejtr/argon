/// Process model skeleton.
///
/// Defines the data structures that will form the basis of argonOS's
/// process management subsystem.  Nothing here runs yet; it serves as the
/// typed foundation for the scheduler and syscall layer.
use crate::future::vfs::Fd;

/// Unique process identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pid(pub u32);

/// Lifecycle state of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Waiting for a CPU time-slice.
    Ready,
    /// Currently executing on a CPU.
    Running,
    /// Blocked on I/O or a synchronisation primitive.
    Blocked,
    /// Terminated; exit code available but resources not yet reclaimed.
    Zombie(i32),
}

/// Maximum number of open file descriptors per process.
const MAX_FDS: usize = 256;

/// Per-process control block.
pub struct Process {
    pub pid: Pid,
    pub state: ProcessState,
    /// Saved register state for context switching (to be populated when
    /// a real scheduler and context-switch assembly are added).
    pub saved_regs: SavedRegisters,
    /// Open file descriptor table.  `None` slots are available.
    pub fd_table: [Option<Fd>; MAX_FDS],
    /// Exit code (valid when `state == Zombie`).
    pub exit_code: i32,
}

impl Process {
    /// Create a new process in the `Ready` state.
    pub const fn new(pid: Pid) -> Self {
        Self {
            pid,
            state: ProcessState::Ready,
            saved_regs: SavedRegisters::zero(),
            fd_table: [None; MAX_FDS],
            exit_code: 0,
        }
    }

    /// Allocate the lowest free file descriptor slot and associate `fd` with it.
    pub fn alloc_fd(&mut self, fd: Fd) -> Option<usize> {
        for (i, slot) in self.fd_table.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(fd);
                return Some(i);
            }
        }
        None
    }

    /// Release a file descriptor slot.
    pub fn free_fd(&mut self, index: usize) -> Option<Fd> {
        self.fd_table.get_mut(index)?.take()
    }
}

/// Saved general-purpose registers for context switching.
///
/// This layout is architecture-specific and will be populated by the
/// context-switch assembly stub once the scheduler is implemented.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct SavedRegisters {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    pub cs: u64,
    pub ss: u64,
}

impl SavedRegisters {
    pub const fn zero() -> Self {
        Self {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            rsp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
            rflags: 0,
            cs: 0,
            ss: 0,
        }
    }
}
