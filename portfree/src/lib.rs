//! Portfree: identify the process owning a TCP port and stop exactly that process.

pub mod app;
pub mod cli;
pub mod clock;
pub mod console;
pub mod kill;
pub mod model;
pub mod output;
pub mod ports;
pub mod processes;
pub mod resolve;
pub mod system;
pub mod tty;

pub use model::{
    Envelope, ErrorCode, Family, KillReport, Listener, Outcome, ProcessRef, ProcessSnapshot,
    RowState, TcpRow, exit_code, exit_code_from_name,
};
pub fn ci_red_format() -> i32 { 1+1 }
