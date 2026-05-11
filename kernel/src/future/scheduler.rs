/// Round-robin cooperative/preemptive scheduler.
///
/// Maintains a fixed-capacity process table.  The timer IRQ calls `on_tick()`
/// every 10 ms; after `TIME_SLICE_TICKS` ticks the current task is
/// preempted and the next ready task is switched in.
///
/// Context switching is done by swapping RSP between task stacks.  The
/// scheduler lock is RELEASED before performing the actual stack swap so that
/// future timer ticks can re-enter `on_tick` without deadlocking.
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::structures::paging::PhysFrame;

use crate::future::process::{Pid, Process, ProcessState};
use crate::serial_println;

/// Number of 10 ms ticks before the current task is preempted.
const TIME_SLICE_TICKS: u64 = 5; // 50 ms per slice

/// Global scheduler instance.
pub static SCHEDULER: Mutex<Option<Scheduler>> = Mutex::new(None);

/// Ticks elapsed in the current time-slice.
static SLICE_TICKS: AtomicU64 = AtomicU64::new(0);

/// Pending user-mode entry RIP — written by the scheduler before the first
/// switch into a user task and consumed by `user_mode_trampoline`.
static PENDING_USER_ENTRY: AtomicU64 = AtomicU64::new(0);
/// Pending user-mode RSP — same lifetime as PENDING_USER_ENTRY.
static PENDING_USER_RSP: AtomicU64 = AtomicU64::new(0);

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
    /// Saved kernel RSP.  `Some` after first initialisation.
    rsp: Option<u64>,
    stack: TaskStack,
    /// CR3 physical frame for user-mode processes (None = kernel task).
    user_cr3: Option<PhysFrame>,
    /// User-mode entry RIP for this task's first switch (user tasks only).
    user_entry: u64,
    /// Initial user-mode RSP for this task's first switch (user tasks only).
    user_rsp_init: u64,
    /// `true` once this task has been switched into at least once.
    started: bool,
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
            user_cr3: None,
            user_entry: 0,
            user_rsp_init: 0,
            started: true, // kernel tasks are ready from spawn
        });

        serial_println!("scheduler: spawned pid={}", pid.0);
        pid
    }

    /// Find the next ready task, update scheduler state, and return the raw
    /// RSP pointers needed for the context switch.
    ///
    /// The caller MUST release the scheduler lock before calling
    /// `context_switch` with the returned pointers; this prevents deadlock
    /// when a timer IRQ fires during the switch.
    ///
    /// Returns `None` if no switch is necessary.
    fn find_and_prepare_switch(&mut self) -> Option<(*mut Option<u64>, Option<u64>)> {
        if self.tasks.len() < 2 {
            return None;
        }
        let len = self.tasks.len();
        let mut next = (self.current + 1) % len;
        for _ in 0..len {
            match self.tasks[next].process.state {
                ProcessState::Ready | ProcessState::Running => break,
                _ => {}
            }
            next = (next + 1) % len;
        }
        if next == self.current {
            return None;
        }

        self.tasks[self.current].process.state = ProcessState::Ready;
        self.tasks[next].process.state = ProcessState::Running;

        let prev = self.current;
        self.current = next;

        // Update TSS.RSP0 and PerCpu kernel_rsp for the incoming task.
        let kstack_top = self.tasks[next].stack.top() as u64;
        crate::arch::x86_64::gdt::set_rsp0(kstack_top);
        crate::arch::x86_64::percpu::set_kernel_rsp(kstack_top);

        // Switch CR3 if the incoming task has its own address space.
        if let Some(cr3) = self.tasks[next].user_cr3 {
            use x86_64::registers::control::{Cr3, Cr3Flags};
            unsafe { Cr3::write(cr3, Cr3Flags::empty()) };
        }

        // Publish user entry info before the first switch into a user task.
        if self.tasks[next].user_cr3.is_some() && !self.tasks[next].started {
            PENDING_USER_ENTRY.store(self.tasks[next].user_entry, Ordering::Release);
            PENDING_USER_RSP.store(self.tasks[next].user_rsp_init, Ordering::Release);
            self.tasks[next].started = true;
        }

        // Return raw pointers — caller releases the lock, then switches.
        let next_rsp = self.tasks[next].rsp;
        let prev_rsp: *mut Option<u64> = &mut self.tasks[prev].rsp;
        Some((prev_rsp, next_rsp))
    }
}

/// Called from the timer IRQ handler.
///
/// Phase 1: acquire the lock, increment the tick counter, decide whether to
/// switch, update scheduler state, and extract the RSP swap parameters.
/// Phase 2: RELEASE the lock, then perform the stack swap.  This two-phase
/// design means the lock is never held across a context switch, so future
/// timer IRQs can safely re-enter `on_tick` without spinning forever.
pub fn on_tick() {
    let switch_info = {
        let mut guard = match SCHEDULER.try_lock() {
            Some(g) => g,
            None => return, // skip tick if scheduler is busy
        };
        let ticks = SLICE_TICKS.fetch_add(1, Ordering::Relaxed);
        if ticks + 1 < TIME_SLICE_TICKS {
            return; // guard dropped, lock released
        }
        SLICE_TICKS.store(0, Ordering::Relaxed);
        match *guard {
            Some(ref mut sched) => sched.find_and_prepare_switch(),
            None => None,
        }
        // `guard` is dropped here — lock is released BEFORE the stack swap.
    };
    if let Some((prev_rsp, next_rsp)) = switch_info {
        // SAFETY: pointers reference live Task stack fields; the lock is
        // released so concurrent timer ticks won't deadlock.
        unsafe { context_switch(prev_rsp, next_rsp) };
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

/// Spawn a kernel task from outside the scheduler.
pub fn spawn(entry: fn() -> !) -> Pid {
    SCHEDULER
        .lock()
        .as_mut()
        .expect("scheduler not initialised")
        .spawn(entry)
}

/// Spawn a user-mode process with its own address space.
///
/// `user_cr3` is the L4 frame for the process, `entry` is the user-space
/// RIP, and `user_rsp` is the initial user-space stack pointer.
///
/// The task's kernel stack is initialised to call `user_mode_trampoline`
/// on the first context switch.  The trampoline reads `PENDING_USER_ENTRY`
/// and `PENDING_USER_RSP` (published by `find_and_prepare_switch`) and
/// performs the one-way `iretq` into ring 3.
pub fn spawn_user_process(user_cr3: PhysFrame, entry: u64, user_rsp: u64) -> Pid {
    let mut guard = SCHEDULER.lock();
    let sched = guard.as_mut().expect("scheduler not initialised");

    let pid = Pid(sched.next_pid);
    sched.next_pid += 1;

    let mut stack = TaskStack::new();
    // First context-switch into this task will `ret` into user_mode_trampoline.
    let rsp = prepare_initial_stack(stack.top(), user_mode_trampoline);

    let mut process = Process::new(pid);
    process.state = ProcessState::Ready;

    sched.tasks.push(Task {
        process,
        rsp: Some(rsp),
        stack,
        user_cr3: Some(user_cr3),
        user_entry: entry,
        user_rsp_init: user_rsp,
        started: false, // find_and_prepare_switch will publish PENDING_* on first switch
    });

    serial_println!("scheduler: spawned user pid={} entry={:#x}", pid.0, entry);
    pid
}

/// Force an immediate task switch (sys_yield implementation).
pub fn force_yield() {
    let switch_info = {
        let mut guard = match SCHEDULER.try_lock() {
            Some(g) => g,
            None => return,
        };
        SLICE_TICKS.store(0, Ordering::Relaxed);
        match *guard {
            Some(ref mut sched) => sched.find_and_prepare_switch(),
            None => None,
        }
    };
    if let Some((prev_rsp, next_rsp)) = switch_info {
        unsafe { context_switch(prev_rsp, next_rsp) };
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
    let switch_info = {
        let mut guard = match SCHEDULER.try_lock() {
            Some(g) => g,
            None => return,
        };
        match *guard {
            Some(ref mut sched) => {
                let cur = sched.current;
                sched.tasks[cur].process.state = ProcessState::Zombie(code);
                serial_println!(
                    "scheduler: pid={} exited code={}",
                    sched.tasks[cur].process.pid.0,
                    code
                );
                SLICE_TICKS.store(0, Ordering::Relaxed);
                sched.find_and_prepare_switch()
            }
            None => None,
        }
    };
    if let Some((prev_rsp, next_rsp)) = switch_info {
        unsafe { context_switch(prev_rsp, next_rsp) };
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
/// Perform the raw RSP swap.
///
/// `prev_rsp` is a raw pointer into the current task's `rsp` field;
/// the scheduler lock must be released by the caller BEFORE this call.
///
/// # Safety
/// * Both stacks must be valid, correctly initialised kernel stacks.
/// * The scheduler lock must NOT be held when this function is called.
#[inline(never)]
unsafe fn context_switch(prev_rsp: *mut Option<u64>, next_rsp: Option<u64>) {
    let next = match next_rsp {
        Some(rsp) => rsp,
        None => return,
    };
    // Get a pointer to the u64 value inside the Option<u64>.
    // SAFETY: the task was spawned with rsp = Some(...), so this is Some.
    let prev_slot: *mut u64 = match &mut *prev_rsp {
        Some(ref mut r) => r as *mut u64,
        None => return,
    };

    asm!(
        // Save callee-saved registers (ABI guarantees caller-saved are not needed).
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // Save current RSP into *prev_slot.
        "mov [{prev}], rsp",
        // Switch to the next task's stack.
        "mov rsp, {next}",
        // Restore next task's callee-saved registers.
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

/// Kernel-side trampoline for user-mode processes.
///
/// This is the first function a user task executes on its kernel stack.
/// It reads the entry RIP and RSP published by `find_and_prepare_switch`
/// and performs the one-way `iretq` into ring 3.  Never returns.
fn user_mode_trampoline() -> ! {
    let entry = PENDING_USER_ENTRY.load(Ordering::Acquire);
    let user_rsp = PENDING_USER_RSP.load(Ordering::Acquire);
    // CR3 was already written by find_and_prepare_switch; read it back so
    // enter_user_mode can write it again (which is a no-op but keeps the
    // API consistent).
    let (cr3_frame, _) = x86_64::registers::control::Cr3::read();
    unsafe { crate::future::usermode::enter_user_mode(entry, user_rsp, cr3_frame) }
}

/// The idle task — runs when no other task is ready.
fn idle_task() -> ! {
    loop {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}
