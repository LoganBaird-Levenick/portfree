//! Console integration: Ctrl+C phases and virtual-terminal color support.

use std::sync::atomic::{AtomicU8, Ordering};

use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Console::{
    ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE,
    STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetConsoleMode,
};
use windows_sys::Win32::System::Threading::ExitProcess;

pub const PHASE_IDLE: u8 = 0;
pub const PHASE_PROMPT: u8 = 1;
pub const PHASE_KILL: u8 = 2;

static PHASE: AtomicU8 = AtomicU8::new(PHASE_IDLE);

pub fn set_phase(phase: u8) {
    PHASE.store(phase, Ordering::SeqCst);
}

pub trait PhaseControl {
    fn prompt(&self);
    fn kill(&self);
    fn idle(&self);
}

pub struct ConsolePhase;

impl PhaseControl for ConsolePhase {
    fn prompt(&self) {
        set_phase(PHASE_PROMPT);
    }

    fn kill(&self) {
        set_phase(PHASE_KILL);
    }

    fn idle(&self) {
        set_phase(PHASE_IDLE);
    }
}

pub struct NoPhase;

impl PhaseControl for NoPhase {
    fn prompt(&self) {}
    fn kill(&self) {}
    fn idle(&self) {}
}

pub fn install() {
    unsafe {
        SetConsoleCtrlHandler(Some(ctrl_handler), 1);
    }
}

pub fn enable_virtual_terminal() {
    for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let handle = unsafe { GetStdHandle(which) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            continue;
        }
        let mut mode = 0;
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
            continue;
        }
        unsafe {
            SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
    }
}

unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> i32 {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};

    if ctrl_type != CTRL_C_EVENT && ctrl_type != CTRL_BREAK_EVENT {
        return 0;
    }
    match PHASE.load(Ordering::SeqCst) {
        PHASE_PROMPT => unsafe { ExitProcess(130) },
        PHASE_KILL => 1, // block the interrupt during the kill batch
        _ => 0,
    }
}
