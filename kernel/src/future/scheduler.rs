/// Round-robin cooperative/preemptive scheduler.
///
/// Maintains a fixed-capacity process table.  The timer IRQ calls `on_tick()`
/// every 10 ms; after `TIME_SLICE_TICKS` ticks the current task is
/// preempted and the next ready task is switched in.
///
/// Context switching is done by swapping RSP between task stacks.  Each task
/// starts life as a kernel function and runs entirely in ring 0 until
/// user-mode support is added.
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::future::process::{Pid, Process, ProcessState};
use crate::serial_println;

/// Number of 10 ms ticks before the current task is preempted.
const TIME_SLICE_TICKS: u64 = 5; // 50 ms per slice

/// Global scheduler instance.
pub static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Ticks elapsed in the current time-slice.
static SLICE_TICKS: AtomicU64 = AtomicU64::new(0);

/// Each kernel task gets a 64 KiB stack.
const TASK_STACK_SIZE: usize = 64 * 1024;

/// Per-task stack storage.  We keep stacks alive as long as the process
/// exists.  `Box<[u8]>` lets us allocate on the heap.
struct TaskStack(alloc::boxed::Box<[u8; TASK_STACK_SIZE]>);

impl TaskStack {
    fn new() -> Self {
        TaskStack(alloc::boxed::Box::new([0u8; TASK_STACK_SIZE]))
    }
    /// Return a pointer to the top of the stack (x86 stacks grow downward).
    fn top(&mut self) -> *mut u8 {
        let base = self.0.as_mut_ptr();
        // SAFETY: we own the allocation and it is TASK_STACK_SIZE bytes.
        unsafe { base.add(TASK_STACK_SIZE) }
    }
}

/// Internal per-task record (augments the public `Process` PCB).
struct Task {
    process: Process,
    /// Saved RSP (None for the idle task / task not yet started).
    rsp: Option<u64>,
    stack: TaskStack,
}

pub struct Scheduler {
    tasks: Vec<Task>,
    current: usize,
    next_pid: u32,
}

impl Scheduler {
    pub fn new() -> Self {
        Scheduler {
            tasks: Vec::new(),
            current: 0,
            next_pid: 1,
        }
    }

    /// Spawn a new kernel task that will call `entry`.
    ///
    /// The task starts in `Ready` state; it will be switched to on the next
    /// preemption boundary.
    pub fn spawn(&mut self, entry: fn() -> !) -> Pid {
        let pid = Pid(self.next_pid);
        self.next_pid += 1;

        let mut stack = TaskStack::new();
        // Set up an initial fake stack frame so the first context switch lands
        // in `entry`.  We push `entry` as the return address.
        let rsp = prepare_initial_stack(stack.top(), entry);

        let mut process = Process::new(pid);
        process.state = ProcessState::Ready;

        self.tasks.push(Task {
            process,
            rsp: Some(rsp),
            stack,
        });

        serial_println!("scheduler: spawned pid={}", pid.0);
        pid
    }

    /// Called from the timer IRQ every tick.  Switches task when the slice
    /// is exhausted.
    pub fn tick(&mut self) {
        let ticks = SLICE_TICKS.fetch_add(1, Ordering::Relaxed);
        if ticks + 1 >= TIME_SLICE_TICKS {
            SLICE_TICKS.store(0, Ordering::Relaxed);
            self.switch_next();
        }
    }

    fn switch_next(&mut self) {
        if self.tasks.len() < 2 {
            return; // nothing to switch to
        }

        // Find the next Ready task.
        let len = self.tasks.len();
        let mut next = (self.current + 1) % len;
        for _ in 0..len {
            if self.tasks[next].process.state == ProcessState::Ready
                || self.tasks[next].process.state == ProcessState::Running
            {
                break;
            }
            next = (next + 1) % len;
        }
        if next == self.current {
            return;
        }

        self.tasks[self.current].process.state = ProcessState::Ready;
        self.tasks[next].process.state = ProcessState::Running;

        let prev = self.current;
        self.current = next;

        // Copy next RSP before mutably borrowing tasks[prev].
        let next_rsp = self.tasks[next].rsp;
        // SAFETY: we hold the scheduler lock during the switch.
        unsafe { context_switch(&mut self.tasks[prev].rsp, next_rsp) };
    }
}

/// Called from the timer IRQ handler.
pub fn on_tick() {
    // Try-lock to avoid deadlock if the IRQ fires while we hold the lock.
    if let Some(ref mut sched) = *SCHEDULER.lock() {
        sched.tick();
    }
}

/// Initialise the global scheduler with an idle task and start it.
pub fn init() {
    let mut sched = Scheduler::new();
    // Idle task: just hlt-loops.
    sched.spawn(idle_task);
    *SCHEDULER.lock() = Some(sched);
    serial_println!("scheduler: initialised");
}

/// Spawn a task from outside the scheduler.
pub fn spawn(entry: fn() -> !) -> Pid {
    SCHEDULER
        .lock()
        .as_mut()
        .expect("scheduler not initialised")
        .spawn(entry)
}

/// Force an immediate task switch (sys_yield implementation).
pub fn force_yield() {
    if let Some(ref mut sched) = *SCHEDULER.lock() {
        sched.switch_next();
    }
}

/// Return the PID of the currently running task.
pub fn current_pid() -> Option<Pid> {
    SCHEDULER
        .lock()
        .as_ref()
        .map(|s| s.tasks[s.current].process.pid)
}

/// Mark the current task as Zombie and switch to the next ready task.
pub fn exit_current(code: i32) {
    if let Some(ref mut sched) = *SCHEDULER.lock() {
        sched.tasks[sched.current].process.state = ProcessState::Zombie(code);
        serial_println!(
            "scheduler: pid={} exited code={}",
            sched.tasks[sched.current].process.pid.0,
            code
        );
        sched.switch_next();
    }
}

// ---------------------------------------------------------------------------
// Low-level helpers
// ---------------------------------------------------------------------------

/// Write an initial stack frame so the task starts at `entry` on first switch.
///
/// x86-64 System V: on entry to a function RSP must be 16-byte aligned
/// *before* the call instruction pushes the return address, i.e. RSP+8 must
/// be 16-byte aligned.  We arrange RSP to point at the `entry` address.
fn prepare_initial_stack(stack_top: *mut u8, entry: fn() -> !) -> u64 {
    // SAFETY: stack_top is valid heap memory we own.
    unsafe {
        let mut rsp = stack_top as u64;
        // Align to 16 bytes then subtract 8 (pre-call convention).
        rsp &= !0xF;
        rsp -= 8;
        // Push entry address as the "return address" for the first ret.
        rsp -= 8;
        *(rsp as *mut u64) = entry as usize as u64;
        rsp
    }
}

/// Save current RSP into `prev_rsp` and load `next_rsp`.
///
/// This is the only unsafe context-switch primitive.  All callee-saved
/// registers (RBX, RBP, R12–R15) are preserved across the `call` that
/// invokes `context_switch` by the Rust calling convention, so we only
/// need to swap RSP.
///
/// # Safety
/// Both RSP values must point to valid, correctly-initialised stacks.
#[inline(never)]
unsafe fn context_switch(prev_rsp: &mut Option<u64>, next_rsp: Option<u64>) {
    let next = match next_rsp {
        Some(rsp) => rsp,
        None => return,
    };
    let prev_slot: *mut u64 = match prev_rsp {
        Some(ref mut r) => r as *mut u64,
        None => return,
    };

    asm!(
        // Save callee-saved registers (the ABI already handles caller-saved).
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // Save current RSP into *prev_slot.
        "mov [{prev}], rsp",
        // Switch to new stack.
        "mov rsp, {next}",
        // Restore new task's callee-saved registers.
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        prev = in(reg) prev_slot,
        next = in(reg) next,
        options(nostack, preserves_flags),
    );
}

/// The idle task — runs when no other task is ready.
fn idle_task() -> ! {
    loop {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}
