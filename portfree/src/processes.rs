//! Process metadata access. `ProcessTable` is the backend seam; the Windows
//! implementation is backed by sysinfo.

use std::time::{SystemTime, UNIX_EPOCH};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::model::{ProcessInfo, ProcessSnapshot};
use crate::ports::TableError;

pub trait ProcessTable {
    fn snapshot(&mut self) -> Result<ProcessSnapshot, TableError>;
}

pub struct SysinfoProcessTable {
    system: System,
    refresh_kind: ProcessRefreshKind,
}

impl SysinfoProcessTable {
    pub fn new() -> Result<Self, TableError> {
        let system = System::new().map_err(|e| TableError(format!("sysinfo: {e}")))?;
        Ok(Self {
            system,
            refresh_kind: ProcessRefreshKind::nothing()
                .with_cmd(UpdateKind::OnlyIfNotSet)
                .with_exe(UpdateKind::OnlyIfNotSet)
                .with_cwd(UpdateKind::OnlyIfNotSet)
                .without_tasks(),
        })
    }
}

impl ProcessTable for SysinfoProcessTable {
    fn snapshot(&mut self) -> Result<ProcessSnapshot, TableError> {
        self.system
            .refresh_processes_specifics(ProcessesToUpdate::All, true, self.refresh_kind);

        let now_unix = unix_now();
        let mut processes = std::collections::BTreeMap::new();
        for (pid, process) in self.system.processes() {
            let pid = pid.as_u32();
            let name = non_empty(process.name().to_string_lossy().into_owned());
            let exe = process
                .exe()
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty());
            let cwd = process
                .cwd()
                .map(|p| p.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty());
            let cmdline = {
                let cmd = process.cmd();
                if cmd.is_empty() {
                    // sysinfo cannot distinguish an empty command line from a
                    // read failure. Report null (unreadable) rather than claiming
                    // the genuinely-empty `[]` case.
                    None
                } else {
                    Some(
                        cmd.iter()
                            .map(|s| s.to_string_lossy().into_owned())
                            .collect(),
                    )
                }
            };
            let started_at_unix = match process.start_time() {
                0 => None,
                seconds => Some(seconds as i64),
            };
            processes.insert(
                pid,
                ProcessInfo {
                    pid,
                    parent: process.parent().map(|p| p.as_u32()),
                    name,
                    exe,
                    cmdline,
                    cwd,
                    started_at_unix,
                },
            );
        }

        Ok(ProcessSnapshot {
            processes,
            now_unix,
        })
    }
}

fn non_empty(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// sysinfo stores whole seconds since the Unix epoch; `GetProcessTimes` is
/// truncated the same way so the identity comparison is exact.
pub fn filetime_to_unix_seconds(filetime: u64) -> i64 {
    const WINDOWS_TO_UNIX_EPOCH_SECS: u64 = 11_644_473_600;
    (filetime / 10_000_000).saturating_sub(WINDOWS_TO_UNIX_EPOCH_SECS) as i64
}
