//! Orchestration: inspect, consent, kill, observe, report.

use std::collections::BTreeSet;
use std::io::BufRead;

use crate::cli::Cli;
use crate::clock::Clock;
use crate::console::PhaseControl;
use crate::kill::{self, KillDeps, KillInput};
use crate::model::{
    Envelope, ErrorCode, ErrorInfo, KillReport, Outcome, ProcessRef, RowState, TargetReport,
};
use crate::output::{self, ColorConfig, Streams, describe_start, sanitize};
use crate::ports::PortTable;
use crate::processes::ProcessTable;
use crate::resolve::{Owner, Resolution, resolve};
use crate::system::ProcessSystem;
use crate::tty::TtyStatus;

pub struct AppDeps<'a> {
    pub ports: &'a mut dyn PortTable,
    pub processes: &'a mut dyn ProcessTable,
    pub system: &'a mut dyn ProcessSystem,
    pub clock: &'a dyn Clock,
    pub stdin: &'a mut dyn BufRead,
    pub streams: Streams<'a>,
    pub tty: TtyStatus,
    pub color: ColorConfig,
    pub phase: &'a dyn PhaseControl,
}

pub fn run(cli: &Cli, deps: &mut AppDeps<'_>) -> i32 {
    let port = cli.port;

    let snapshot = match deps.processes.snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => {
            let envelope = Envelope::refused(port, ErrorCode::QueryFailed, Vec::new());
            return finish(cli, &envelope, deps, false);
        }
    };
    let rows = match deps.ports.tcp_rows() {
        Ok(rows) => rows,
        Err(_) => {
            let envelope = Envelope::refused(port, ErrorCode::QueryFailed, Vec::new());
            return finish(cli, &envelope, deps, false);
        }
    };

    let mut envelope = Envelope::new(port);
    let resolved = match resolve(port, &rows, &snapshot, cli.pid) {
        Resolution::NoListener => {
            return finish(cli, &Envelope::no_listener(port), deps, false);
        }
        Resolution::Refused {
            code,
            listeners,
            pid,
        } => {
            envelope.listeners = listeners;
            envelope.kill.outcome = Outcome::NotAttempted;
            envelope.error = Some(ErrorInfo::for_code(code, port, pid.or(cli.pid)));
            return finish(cli, &envelope, deps, false);
        }
        Resolution::Resolved(resolved) => resolved,
    };

    envelope.listeners = resolved.listeners;
    envelope.warnings = resolved.warnings;

    let target = resolved.target;
    let query = match deps.system.open_query(target.pid) {
        Ok(query) => query,
        Err(_) => {
            return not_attempted(
                cli,
                envelope,
                deps,
                ErrorCode::NotInspectable,
                Some(target.pid),
            );
        }
    };

    match deps.system.creation_time_unix(query) {
        Ok(started) if started == target.started_at_unix => {}
        Ok(_) => {
            deps.system.close_query(query);
            return not_attempted(
                cli,
                envelope,
                deps,
                ErrorCode::StaleTarget,
                Some(target.pid),
            );
        }
        Err(_) => {
            deps.system.close_query(query);
            return not_attempted(
                cli,
                envelope,
                deps,
                ErrorCode::NotInspectable,
                Some(target.pid),
            );
        }
    }

    if !deps.system.is_alive(query) {
        deps.system.close_query(query);
        return not_attempted(
            cli,
            envelope,
            deps,
            ErrorCode::StaleTarget,
            Some(target.pid),
        );
    }

    if let Err(code) = deps.system.safety(target.pid, query, &snapshot) {
        deps.system.close_query(query);
        return not_attempted(cli, envelope, deps, code, Some(target.pid));
    }

    envelope.target = Some(TargetReport {
        pid: target.pid,
        started_at_unix: target.started_at_unix,
        other_listening_ports: resolved.other_listening_ports.clone(),
    });

    if !cli.json {
        let _ = output::write_warnings(&envelope.warnings, deps.streams.stderr, deps.color.stderr);
    }

    if cli.dry_run_effective() {
        deps.system.close_query(query);
        envelope.kill = kill::dry_run_report(
            resolved.kill_set.clone(),
            &resolved.preexisting,
            &resolved.kill_set,
        );
        return finish(cli, &envelope, deps, false);
    }

    if cli.prompt_required() && !deps.tty.interactive() {
        deps.system.close_query(query);
        return not_attempted(cli, envelope, deps, ErrorCode::NoTty, Some(target.pid));
    }

    if cli.prompt_required() {
        let target_listener = envelope.listeners.iter().find(|l| l.pid == target.pid);
        let descendants = resolved.kill_set.len().saturating_sub(1);
        deps.phase.prompt();
        let consented = prompt(deps, port, target_listener, descendants);
        deps.phase.idle();
        if !consented {
            deps.system.close_query(query);
            envelope.kill = kill::dry_run_report(
                resolved.kill_set.clone(),
                &resolved.preexisting,
                &resolved.kill_set,
            );
            return finish(cli, &envelope, deps, true);
        }
    }

    deps.phase.kill();
    let fresh = match deps.processes.snapshot() {
        Ok(fresh) => fresh,
        Err(_) => {
            deps.system.close_query(query);
            deps.phase.idle();
            return aborted(
                cli,
                envelope,
                deps,
                ErrorCode::AccessDenied,
                "could not re-read the process table before killing".into(),
                resolved.kill_set.clone(),
            );
        }
    };

    let preexisting = match deps.ports.tcp_rows() {
        Ok(rows) => owners_on_port(port, &rows, &fresh),
        Err(_) => resolved.preexisting.clone(),
    };

    let mut kill_set = vec![target];
    for pid in fresh.descendants_of(target.pid) {
        let Some(started_at_unix) = fresh.get(pid).and_then(|p| p.started_at_unix) else {
            deps.system.close_query(query);
            deps.phase.idle();
            return aborted(
                cli,
                envelope,
                deps,
                ErrorCode::AccessDenied,
                format!("PID {pid} could not be inspected"),
                resolved.kill_set.clone(),
            );
        };
        kill_set.push(ProcessRef {
            pid,
            started_at_unix,
        });
    }

    let report = kill::execute(
        KillInput {
            port,
            target,
            kill_set,
            preexisting,
            target_query: query,
            snapshot: fresh,
        },
        &mut KillDeps {
            ports: deps.ports,
            processes: deps.processes,
            system: deps.system,
            clock: deps.clock,
        },
    );
    deps.phase.idle();

    envelope.target = Some(TargetReport {
        pid: target.pid,
        started_at_unix: target.started_at_unix,
        other_listening_ports: resolved.other_listening_ports,
    });
    envelope.kill = report;
    finish(cli, &envelope, deps, false)
}

fn not_attempted(
    cli: &Cli,
    mut envelope: Envelope,
    deps: &mut AppDeps<'_>,
    code: ErrorCode,
    pid: Option<u32>,
) -> i32 {
    envelope.target = None;
    envelope.kill = KillReport {
        outcome: Outcome::NotAttempted,
        ..KillReport::default()
    };
    envelope.error = Some(ErrorInfo::for_code(code, cli.port, pid));
    finish(cli, &envelope, deps, false)
}

fn aborted(
    cli: &Cli,
    mut envelope: Envelope,
    deps: &mut AppDeps<'_>,
    code: ErrorCode,
    message: String,
    would_kill: Vec<ProcessRef>,
) -> i32 {
    envelope.kill = KillReport {
        outcome: Outcome::Aborted,
        would_kill,
        error: Some(ErrorInfo { code, message }),
        ..KillReport::default()
    };
    finish(cli, &envelope, deps, false)
}

fn owners_on_port(
    port: u16,
    rows: &[crate::model::TcpRow],
    snapshot: &crate::model::ProcessSnapshot,
) -> Vec<Owner> {
    let mut pids = BTreeSet::new();
    for row in rows
        .iter()
        .filter(|row| row.port == port && row.state == RowState::Listen)
    {
        for pid in row.pids.iter().copied().filter(|pid| *pid != 0) {
            pids.insert(pid);
        }
    }
    pids.into_iter()
        .map(|pid| Owner {
            pid,
            started_at_unix: snapshot
                .get(pid)
                .and_then(|process| process.started_at_unix),
        })
        .collect()
}

fn prompt(
    deps: &mut AppDeps<'_>,
    port: u16,
    target: Option<&crate::model::Listener>,
    descendants: usize,
) -> bool {
    let (pid, name, exe, cwd, started) = match target {
        Some(listener) => (
            listener.pid.to_string(),
            sanitize(listener.name.as_deref().unwrap_or("unknown")),
            sanitize(listener.exe.as_deref().unwrap_or("unknown")),
            sanitize(listener.cwd.as_deref().unwrap_or("unknown")),
            describe_start(listener.started_at_unix, listener.age_seconds),
        ),
        None => (
            "unknown".into(),
            "unknown".into(),
            "unknown".into(),
            "unknown".into(),
            "unknown".into(),
        ),
    };

    let _ = writeln!(
        deps.streams.stderr,
        "\nStop PID {pid} ({name}) on port {port}?"
    );
    let _ = writeln!(deps.streams.stderr, "  exe:         {exe}");
    let _ = writeln!(deps.streams.stderr, "  cwd:         {cwd}");
    let _ = writeln!(deps.streams.stderr, "  started:     {started}");
    let _ = writeln!(deps.streams.stderr, "  descendants: {descendants}");
    let _ = writeln!(
        deps.streams.stderr,
        "This force-terminates the process and its descendants."
    );
    let _ = write!(deps.streams.stderr, "Confirm [y/N]: ");
    let _ = deps.streams.stderr.flush();

    let mut line = String::new();
    match deps.stdin.read_line(&mut line) {
        Ok(0) => false,
        Ok(_) => {
            let answer = line.trim();
            answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
        }
        Err(_) => false,
    }
}

fn finish(cli: &Cli, envelope: &Envelope, deps: &mut AppDeps<'_>, declined: bool) -> i32 {
    output::write_output(cli.json, envelope, &mut deps.streams, deps.color, declined)
}
