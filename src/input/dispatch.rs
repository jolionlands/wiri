//! Hook callbacks only enqueue work. All actions and drag work run on the existing hotkey
//! message thread; no worker per event, extra input thread, or idle wake-up is needed.
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_USER};

use super::Action;

pub const WM_INPUT_WORK: u32 = WM_USER + 305;
pub enum Work {
    Action(Action),
    /// A `unified-grid` arrow chord: the two arrow presses were merged into one diagonal
    /// action, so the engine continues from the state the first arrow started on.
    ChordedAction(Action),
    Recent(super::recent::RecentInput),
    Task(Box<dyn FnOnce() + Send>),
}
static THREAD: AtomicU32 = AtomicU32::new(0);
static QUEUE: Mutex<VecDeque<Work>> = Mutex::new(VecDeque::new());

pub fn initialize(thread: u32) {
    THREAD.store(thread, Ordering::Release);
}
pub fn shutdown() {
    THREAD.store(0, Ordering::Release);
    QUEUE.lock().clear();
}
fn enqueue(work: Work) -> bool {
    let thread = THREAD.load(Ordering::Acquire);
    if thread == 0 {
        return false;
    }
    let mut queue = QUEUE.lock();
    let wake = queue.is_empty();
    queue.push_back(work);
    if wake && unsafe { PostThreadMessageW(thread, WM_INPUT_WORK, WPARAM(0), LPARAM(0)) }.is_err() {
        queue.clear();
        return false;
    }
    true
}
pub fn is_initialized() -> bool {
    THREAD.load(Ordering::Acquire) != 0
}
pub fn send_recent(input: super::recent::RecentInput) -> bool {
    enqueue(Work::Recent(input))
}
pub fn send(action: Action) -> bool {
    enqueue(Work::Action(action))
}
/// Queue the second half of a `unified-grid` arrow chord.
pub fn send_chorded(action: Action) -> bool {
    enqueue(Work::ChordedAction(action))
}
pub fn send_work(work: Box<dyn FnOnce() + Send>) -> bool {
    enqueue(Work::Task(work))
}
pub fn take() -> VecDeque<Work> {
    std::mem::take(&mut *QUEUE.lock())
}
