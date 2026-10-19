//! # Green Thread Scheduler (riscv64)
//!
//! Cooperative green-thread scheduler built on top of context switching.
//! This crate is **riscv64 only**.

#![cfg(target_arch = "riscv64")]

use core::arch::naked_asm;

const STACK_SIZE: usize = 1024 * 128;

#[repr(C)]
#[derive(Debug, Default, Clone)]
pub struct TaskContext {
    sp: u64, ra: u64,
    s0: u64, s1: u64, s2: u64, s3: u64,
    s4: u64, s5: u64, s6: u64, s7: u64,
    s8: u64, s9: u64, s10: u64, s11: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThreadState { Ready, Running, Finished }

struct GreenThread {
    ctx: TaskContext,
    state: ThreadState,
    _stack: Option<Vec<u8>>,
    entry: Option<extern "C" fn()>,
}

static mut CURRENT_THREAD_ENTRY: Option<extern "C" fn()> = None;

extern "C" fn thread_wrapper() {
    let entry = unsafe { core::ptr::read(&raw const CURRENT_THREAD_ENTRY) };
    if let Some(f) = entry {
        unsafe { CURRENT_THREAD_ENTRY = None };
        f();
    }
    thread_finished();
}

#[unsafe(naked)]
unsafe extern "C" fn switch_context(_old: &mut TaskContext, _new: &TaskContext) {
    naked_asm!(
        "sd sp, 0(a0)",   "sd ra, 8(a0)",
        "sd s0, 16(a0)",  "sd s1, 24(a0)",  "sd s2, 32(a0)",  "sd s3, 40(a0)",
        "sd s4, 48(a0)",  "sd s5, 56(a0)",  "sd s6, 64(a0)",  "sd s7, 72(a0)",
        "sd s8, 80(a0)",  "sd s9, 88(a0)",  "sd s10, 96(a0)", "sd s11, 104(a0)",
        "ld sp, 0(a1)",   "ld ra, 8(a1)",
        "ld s0, 16(a1)",  "ld s1, 24(a1)",  "ld s2, 32(a1)",  "ld s3, 40(a1)",
        "ld s4, 48(a1)",  "ld s5, 56(a1)",  "ld s6, 64(a1)",  "ld s7, 72(a1)",
        "ld s8, 80(a1)",  "ld s9, 88(a1)",  "ld s10, 96(a1)", "ld s11, 104(a1)",
        "li a0, 0", "li a1, 0", "ret",
    );
}

pub struct Scheduler {
    threads: Vec<GreenThread>,
    current: usize,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            threads: vec![GreenThread {
                ctx: TaskContext::default(),
                state: ThreadState::Running,
                _stack: None,
                entry: None,
            }],
            current: 0,
        }
    }

    pub fn spawn(&mut self, entry: extern "C" fn()) {
        let stack = vec![0u8; STACK_SIZE];
        let stack_top = stack.as_ptr() as usize + STACK_SIZE;
        let sp = stack_top & !15;        // 16 字节对齐防御

        let mut ctx = TaskContext::default();
        ctx.sp = sp as u64;
        ctx.ra = thread_wrapper as u64;

        self.threads.push(GreenThread {
            ctx,
            state: ThreadState::Ready,
            _stack: Some(stack),
            entry: Some(entry),
        });
    }

    pub fn run(&mut self) {
        unsafe { SCHEDULER = self as *mut Scheduler; }

        loop {
            let all_finished = self.threads[1..]
                .iter()
                .all(|t| t.state == ThreadState::Finished);
            if all_finished { break; }
            self.schedule_next();
        }

        unsafe { SCHEDULER = std::ptr::null_mut(); }
    }

    fn schedule_next(&mut self) {
        if self.threads[self.current].state != ThreadState::Finished {
            self.threads[self.current].state = ThreadState::Ready;
        }

        let n = self.threads.len();
        let mut next = (self.current + 1) % n;
        let start = next;
        loop {
            if self.threads[next].state == ThreadState::Ready { break; }
            next = (next + 1) % n;
            if next == start { return; }
        }

        let old_idx = self.current;
        let new_idx = next;

        self.threads[new_idx].state = ThreadState::Running;
        self.current = new_idx;
        if let Some(entry) = self.threads[new_idx].entry {
            unsafe { CURRENT_THREAD_ENTRY = Some(entry); }
        }

        unsafe {
            let old_ptr: *mut TaskContext   = &mut self.threads[old_idx].ctx;
            let new_ptr: *const TaskContext = &self.threads[new_idx].ctx;
            switch_context(&mut *old_ptr, &*new_ptr);
        }
    }
}

static mut SCHEDULER: *mut Scheduler = std::ptr::null_mut();

pub fn yield_now() {
    unsafe {
        if !SCHEDULER.is_null() {
            (*SCHEDULER).schedule_next();
        }
    }
}

fn thread_finished() {
    unsafe {
        if !SCHEDULER.is_null() {
            let sched = &mut *SCHEDULER;
            sched.threads[sched.current].state = ThreadState::Finished;
            sched.schedule_next();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static EXEC_ORDER: AtomicU32 = AtomicU32::new(0);

    extern "C" fn task_a() {
        EXEC_ORDER.fetch_add(1, Ordering::SeqCst);
        yield_now();
        EXEC_ORDER.fetch_add(10, Ordering::SeqCst);
        yield_now();
        EXEC_ORDER.fetch_add(100, Ordering::SeqCst);
    }

    extern "C" fn task_b() {
        EXEC_ORDER.fetch_add(1, Ordering::SeqCst);
        yield_now();
        EXEC_ORDER.fetch_add(10, Ordering::SeqCst);
    }

    #[test]
    fn test_scheduler_runs_all() {
        let _guard = TEST_LOCK.lock().unwrap();
        EXEC_ORDER.store(0, Ordering::SeqCst);

        let mut sched = Scheduler::new();
        sched.spawn(task_a);
        sched.spawn(task_b);
        sched.run();

        let got = EXEC_ORDER.load(Ordering::SeqCst);
        if got != 122 {
            panic!("EXEC_ORDER: expected 122, got {}", got);
        }
    }

    static SIMPLE_FLAG: AtomicU32 = AtomicU32::new(0);

    extern "C" fn simple_task() {
        SIMPLE_FLAG.store(42, Ordering::SeqCst);
    }

    #[test]
    fn test_single_thread() {
        let _guard = TEST_LOCK.lock().unwrap();
        SIMPLE_FLAG.store(0, Ordering::SeqCst);

        let mut sched = Scheduler::new();
        sched.spawn(simple_task);
        sched.run();

        assert_eq!(SIMPLE_FLAG.load(Ordering::SeqCst), 42);
    }
}