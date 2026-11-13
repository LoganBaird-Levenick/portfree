//! Domain types shared by the resolver, safety checks, kill executor, and renderers.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::Serialize;

pub const SCHEMA_VERSION: u32 = 1;

pub const STATEFUL_EXES: &[&str] = &[
    "postgres.exe",
    "mysqld.exe",
    "sqlservr.exe",
    "mongod.exe",
    "redis-server.exe",
    "com.docker.backend.exe",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    Ipv4,
    Ipv6,
}

impl Family {
    pub fn of(address: IpAddr) -> Self {
        match address {
            IpAddr::V4(_) => Self::Ipv4,
            IpAddr::V6(_) => Self::Ipv6,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Listen,
    Other,
}

/// One row of the native TCP table. This is the seam a non-Windows backend attaches to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpRow {
    pub family: Family,
    pub address: IpAddr,
    pub port: u16,
    pub state: RowState,
    pub pids: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: Option<String>,
    pub exe: Option<String>,
    pub cmdline: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub started_at_unix: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub processes: BTreeMap<u32, ProcessInfo>,
    pub now_unix: i64,
}

impl ProcessSnapshot {
    pub fn get(&self, pid: u32) -> Option<&ProcessInfo> {
        self.processes.get(&pid)
    }

    /// Direct children, sorted for deterministic output.
    pub fn children_of(&self, pid: u32) -> Vec<u32> {
        let mut children: Vec<u32> = self
            .processes
            .values()
            .filter(|p| p.parent == Some(pid))
            .map(|p| p.pid)
            .collect();
        children.sort_unstable();
        children.dedup();
        children
    }

    /// Direct and indirect descendants, breadth-first, excluding `pid` itself.
    pub fn descendants_of(&self, pid: u32) -> Vec<u32> {
        let mut seen = std::collections::BTreeSet::new();
        let mut order = Vec::new();
        let mut queue = std::collections::VecDeque::from([pid]);
        while let Some(parent) = queue.pop_front() {
            for child in self.children_of(parent) {
                if child != pid && seen.insert(child) {
                    order.push(child);
                    queue.push_back(child);
                }
            }
        }
        order
    }

    /// Parent chain, nearest ancestor first, excluding `pid` itself.
    pub fn ancestors_of(&self, pid: u32) -> Vec<u32> {
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        let mut current = self.get(pid).and_then(|p| p.parent);
        while let Some(parent) = current {
            if !seen.insert(parent) {
                break;
            }
            out.push(parent);
            current = self.get(parent).and_then(|p| p.parent);
        }
        out
    }

    pub fn identity(&self, pid: u32) -> Option<ProcessRef> {
        self.get(pid)
            .and_then(|p| p.started_at_unix)
            .map(|started_at_unix| ProcessRef {
                pid,
                started_at_unix,
            })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ProcessRef {
    pub pid: u32,
    pub started_at_unix: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Listener {
    pub protocol: String,
    pub family: Family,
    pub address: String,
    pub port: u16,
    pub pid: u32,
    pub name: Option<String>,
    pub exe: Option<String>,
    pub cmdline: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub started_at_unix: Option<i64>,
    pub age_seconds: Option<i64>,
    pub parent: Option<u32>,
    pub children: Vec<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetReport {
    pub pid: u32,
    pub started_at_unix: i64,
    pub other_listening_ports: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Respawned {
    pub pid: u32,
    pub started_at_unix: Option<i64>,
    pub name: Option<String>,
    pub exe: Option<String>,
    pub cmdline: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
    pub pid: u32,
}

impl Warning {
    pub fn stateful(pid: u32, name: &str) -> Self {
        Self {
            code: "stateful_process".into(),
            message: format!(
                "{name} is a stateful process; terminating it may lose unflushed data"
            ),
            pid,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub message: String,
}

impl ErrorInfo {
    pub fn for_code(code: ErrorCode, port: u16, pid: Option<u32>) -> Self {
        Self {
            code,
            message: code.message(port, pid),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    NoListener,
    DryRun,
    #[default]
    NotAttempted,
    TargetExited,
    Killed,
    Aborted,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    AmbiguousPort,
    PidNotListening,
    PidZero,
    CriticalProcess,
    SessionZero,
    ServiceAccount,
    OtherUser,
    WindowsDirectory,
    AncestorProcess,
    NotInspectable,
    QueryFailed,
    NoTty,
    StaleTarget,
    AccessDenied,
    Survivor,
    Respawned,
    PortStillHeld,
    KillFailed,
}

impl ErrorCode {
    pub fn parse(code: &str) -> Option<Self> {
        Some(match code {
            "ambiguous_port" => Self::AmbiguousPort,
            "pid_not_listening" => Self::PidNotListening,
            "pid_zero" => Self::PidZero,
            "critical_process" => Self::CriticalProcess,
            "session_zero" => Self::SessionZero,
            "service_account" => Self::ServiceAccount,
            "other_user" => Self::OtherUser,
            "windows_directory" => Self::WindowsDirectory,
            "ancestor_process" => Self::AncestorProcess,
            "not_inspectable" => Self::NotInspectable,
            "query_failed" => Self::QueryFailed,
            "no_tty" => Self::NoTty,
            "stale_target" => Self::StaleTarget,
            "access_denied" => Self::AccessDenied,
            "survivor" => Self::Survivor,
            "respawned" => Self::Respawned,
            "port_still_held" => Self::PortStillHeld,
            "kill_failed" => Self::KillFailed,
            _ => return None,
        })
    }

    pub fn exit_code(self) -> i32 {
        match self {
            Self::NoTty => 4,
            Self::Survivor | Self::Respawned | Self::PortStillHeld | Self::KillFailed => 1,
            _ => 3,
        }
    }

    pub fn message(self, port: u16, pid: Option<u32>) -> String {
        let pid = pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into());
        match self {
            Self::AmbiguousPort => {
                format!("port {port} has more than one owner; re-run with --pid to choose one")
            }
            Self::PidNotListening => format!("PID {pid} is not listening on port {port}"),
            Self::PidZero => format!("port {port} has a listener with no owning process"),
            Self::CriticalProcess => format!("PID {pid} is a critical Windows process"),
            Self::SessionZero => format!("PID {pid} runs in session 0"),
            Self::ServiceAccount => format!("PID {pid} runs as a service account"),
            Self::OtherUser => format!("PID {pid} belongs to another user"),
            Self::WindowsDirectory => {
                format!("PID {pid} runs from the Windows directory")
            }
            Self::AncestorProcess => {
                format!("PID {pid} is portfree or one of its ancestors")
            }
            Self::NotInspectable => format!("PID {pid} could not be inspected"),
            Self::QueryFailed => "could not read the TCP or process table".into(),
            Self::NoTty => {
                "an interactive terminal is required; use --yes, --dry-run, or --json".into()
            }
            Self::StaleTarget => {
                format!("PID {pid} is no longer the process that was inspected")
            }
            Self::AccessDenied => format!("access denied while preparing to stop PID {pid}"),
            Self::Survivor => "one or more processes did not exit".into(),
            Self::Respawned => "the port was rebound during the observation window".into(),
            Self::PortStillHeld => format!("port {port} is still held"),
            Self::KillFailed => "TerminateProcess failed".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct KillReport {
    pub outcome: Outcome,
    pub would_kill: Vec<ProcessRef>,
    pub killed: Vec<ProcessRef>,
    pub survivors: Vec<ProcessRef>,
    pub respawned: Option<Vec<Respawned>>,
    pub port_held_by: Option<Vec<ProcessRef>>,
    pub port_free: Option<bool>,
    pub error: Option<ErrorInfo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Envelope {
    pub schema_version: u32,
    pub port: u16,
    pub listeners: Vec<Listener>,
    pub target: Option<TargetReport>,
    pub kill: KillReport,
    pub warnings: Vec<Warning>,
    pub error: Option<ErrorInfo>,
}

impl Envelope {
    pub fn new(port: u16) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            port,
            listeners: Vec::new(),
            target: None,
            kill: KillReport::default(),
            warnings: Vec::new(),
            error: None,
        }
    }

    pub fn refused(port: u16, code: ErrorCode, listeners: Vec<Listener>) -> Self {
        let mut envelope = Self::new(port);
        envelope.listeners = listeners;
        envelope.kill.outcome = Outcome::NotAttempted;
        envelope.kill.port_held_by = None;
        envelope.error = Some(ErrorInfo::for_code(code, port, None));
        envelope
    }

    pub fn no_listener(port: u16) -> Self {
        let mut envelope = Self::new(port);
        envelope.kill.outcome = Outcome::NoListener;
        envelope.kill.respawned = None;
        envelope.kill.port_held_by = Some(Vec::new());
        envelope.kill.port_free = Some(true);
        envelope
    }

    pub fn exit_code(&self) -> i32 {
        exit_code(self.kill.outcome, self.error.as_ref().map(|e| e.code))
    }
}

/// Exit code for a known outcome, falling back to the outcome when a consumer
/// sees an error code this version does not know.
pub fn exit_code(outcome: Outcome, code: Option<ErrorCode>) -> i32 {
    match outcome {
        Outcome::NotAttempted => code.map_or(3, ErrorCode::exit_code),
        Outcome::Aborted => 3,
        Outcome::Failed => 1,
        Outcome::NoListener | Outcome::DryRun | Outcome::TargetExited | Outcome::Killed => 0,
    }
}

/// Consumer helper: an unknown `code` (for example from a newer minor version)
/// falls back to the outcome's exit code.
pub fn exit_code_from_name(outcome: Outcome, code: Option<&str>) -> i32 {
    exit_code(outcome, code.and_then(ErrorCode::parse))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_error_codes_map_to_their_exit_codes() {
        assert_eq!(exit_code(Outcome::NotAttempted, Some(ErrorCode::NoTty)), 4);
        assert_eq!(
            exit_code(Outcome::NotAttempted, Some(ErrorCode::AmbiguousPort)),
            3
        );
        assert_eq!(
            exit_code(Outcome::Aborted, Some(ErrorCode::AccessDenied)),
            3
        );
        assert_eq!(exit_code(Outcome::Failed, Some(ErrorCode::Respawned)), 1);
    }

    #[test]
    fn an_unknown_code_falls_back_to_the_outcome() {
        assert_eq!(
            exit_code_from_name(Outcome::NotAttempted, Some("future_code")),
            3
        );
        assert_eq!(exit_code_from_name(Outcome::Failed, Some("future_code")), 1);
        assert_eq!(exit_code_from_name(Outcome::Killed, Some("future_code")), 0);
        assert_eq!(exit_code_from_name(Outcome::NotAttempted, None), 3);
    }
}
