//! Human and JSON rendering. Human output is the sanitization boundary.

use std::collections::BTreeMap;
use std::io::{self, Write};

use crate::model::{Envelope, ErrorInfo, Listener, Outcome, ProcessRef, Warning};

/// Strip C0 controls (except tab) and DEL. Process-derived strings only reach
/// the human boundary through this function; JSON keeps the raw value.
pub fn sanitize(value: &str) -> String {
    value
        .chars()
        .filter(|ch| *ch == '\t' || (*ch >= ' ' && *ch != '\u{7f}'))
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ColorConfig {
    pub stdout: bool,
    pub stderr: bool,
}

pub struct Streams<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}

pub fn write_json(envelope: &Envelope, out: &mut dyn Write) -> io::Result<()> {
    serde_json::to_writer(&mut *out, envelope)?;
    writeln!(out)
}

/// Write the one output document for a run and return its exit code.
pub fn write_output(
    json: bool,
    envelope: &Envelope,
    streams: &mut Streams<'_>,
    color: ColorConfig,
    declined: bool,
) -> i32 {
    let code = envelope.exit_code();
    if json {
        let _ = write_json(envelope, streams.stdout);
    } else {
        let _ = write_human(envelope, streams, color, declined);
    }
    let _ = streams.stdout.flush();
    let _ = streams.stderr.flush();
    code
}

pub fn write_warnings(warnings: &[Warning], out: &mut dyn Write, color: bool) -> io::Result<()> {
    for warning in warnings {
        write_warning(warning, out, color)?;
    }
    out.flush()
}

pub fn write_human(
    envelope: &Envelope,
    streams: &mut Streams<'_>,
    color: ColorConfig,
    declined: bool,
) -> io::Result<()> {
    write_report(envelope, streams.stdout, color.stdout)?;

    if let Some(error) = &envelope.error {
        write_error(error, streams.stderr, color.stderr)?;
        writeln!(streams.stdout, "No processes were stopped.")?;
        return Ok(());
    }

    write_result(envelope, streams.stdout, color.stdout, declined)
}

fn write_report(envelope: &Envelope, out: &mut dyn Write, color: bool) -> io::Result<()> {
    if envelope.listeners.is_empty() {
        return Ok(());
    }

    let groups = group_listeners(&envelope.listeners);
    let listener_count: usize = envelope.listeners.len();
    let process_count = groups.len();

    writeln!(
        out,
        "Port {} is held by {} process{} ({} listener{}).",
        envelope.port,
        process_count,
        plural(process_count),
        listener_count,
        s(listener_count)
    )?;

    for (pid, listeners) in groups {
        let first = &listeners[0];
        writeln!(
            out,
            "\n  {}  {}",
            bold(&format!("PID {pid}"), color),
            sanitize(first.name.as_deref().unwrap_or("unknown"))
        )?;
        writeln!(out, "    exe:      {}", field(first.exe.as_deref()))?;
        writeln!(out, "    cwd:      {}", field(first.cwd.as_deref()))?;
        writeln!(
            out,
            "    started:  {}",
            describe_start(first.started_at_unix, first.age_seconds)
        )?;
        writeln!(
            out,
            "    parent:   {}",
            first
                .parent
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".into())
        )?;
        writeln!(
            out,
            "    children: {}",
            if first.children.is_empty() {
                "none".to_string()
            } else {
                first
                    .children
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )?;
        writeln!(out, "    listeners:")?;
        for listener in listeners {
            writeln!(
                out,
                "      {} {} {}",
                listener.protocol,
                listener.family.as_str(),
                listener_address(listener)
            )?;
        }
    }
    writeln!(out)?;
    Ok(())
}

fn write_warning(warning: &Warning, out: &mut dyn Write, color: bool) -> io::Result<()> {
    writeln!(
        out,
        "{} {}",
        paint("warning:", "\x1b[33m", color),
        sanitize(&warning.message)
    )
}

fn write_error(error: &ErrorInfo, out: &mut dyn Write, color: bool) -> io::Result<()> {
    writeln!(
        out,
        "{} {}",
        paint("error:", "\x1b[31m", color),
        sanitize(&error.message)
    )
}

fn write_result(
    envelope: &Envelope,
    out: &mut dyn Write,
    color: bool,
    declined: bool,
) -> io::Result<()> {
    let port = envelope.port;
    match envelope.kill.outcome {
        Outcome::NoListener => {
            writeln!(
                out,
                "{}",
                paint(&format!("Port {port} is free."), "\x1b[32m", color)
            )
        }
        Outcome::DryRun => {
            let count = envelope.kill.would_kill.len();
            if declined {
                writeln!(
                    out,
                    "Declined: no processes were stopped. Port {port} was not touched."
                )
            } else {
                writeln!(
                    out,
                    "Dry run: would stop {count} process{}. Port {port} was not touched.",
                    plural(count)
                )
            }
        }
        Outcome::TargetExited => {
            let pid = envelope
                .target
                .as_ref()
                .map(|t| t.pid.to_string())
                .unwrap_or_else(|| "target".into());
            writeln!(
                out,
                "{}",
                paint(
                    &format!("PID {pid} had already exited. Port {port} is free."),
                    "\x1b[32m",
                    color
                )
            )
        }
        Outcome::Killed => {
            let count = envelope.kill.killed.len();
            writeln!(
                out,
                "{}",
                paint(
                    &format!(
                        "Stopped {count} process{}. Port {port} is free.",
                        plural(count)
                    ),
                    "\x1b[32m",
                    color
                )
            )
        }
        Outcome::Aborted => {
            let message = envelope
                .kill
                .error
                .as_ref()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "aborted".into());
            writeln!(
                out,
                "{} Nothing was stopped.",
                paint(
                    &format!("Aborted: {}", sanitize(&message)),
                    "\x1b[31m",
                    color
                )
            )
        }
        Outcome::Failed => {
            let message = envelope
                .kill
                .error
                .as_ref()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "failed".into());
            writeln!(
                out,
                "{}",
                paint(
                    &format!("Failed: {}", sanitize(&message)),
                    "\x1b[31m",
                    color
                )
            )?;
            if !envelope.kill.survivors.is_empty() {
                writeln!(
                    out,
                    "  survivors: {}",
                    process_list(&envelope.kill.survivors)
                )?;
            }
            if let Some(respawned) = &envelope.kill.respawned
                && !respawned.is_empty()
            {
                let holders = respawned
                    .iter()
                    .map(|r| {
                        format!(
                            "{} ({})",
                            r.pid,
                            sanitize(r.name.as_deref().unwrap_or("unknown"))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(out, "  new holder: {holders}")?;
            }
            if let Some(held) = &envelope.kill.port_held_by
                && !held.is_empty()
            {
                writeln!(out, "  still held by: {}", process_list(held))?;
            }
            if envelope.kill.port_free == Some(true) {
                writeln!(out, "Port {port} is free.")?;
            }
            Ok(())
        }
        Outcome::NotAttempted => {
            writeln!(out, "No processes were stopped.")?;
            Ok(())
        }
    }
}

fn group_listeners(listeners: &[Listener]) -> BTreeMap<u32, Vec<&Listener>> {
    let mut groups: BTreeMap<u32, Vec<&Listener>> = BTreeMap::new();
    for listener in listeners {
        groups.entry(listener.pid).or_default().push(listener);
    }
    groups
}

fn listener_address(listener: &Listener) -> String {
    match listener.family {
        crate::model::Family::Ipv4 => format!("{}:{}", listener.address, listener.port),
        crate::model::Family::Ipv6 => format!("[{}]:{}", listener.address, listener.port),
    }
}

fn process_list(processes: &[ProcessRef]) -> String {
    processes
        .iter()
        .map(|p| format!("{} (started {})", p.pid, p.started_at_unix))
        .collect::<Vec<_>>()
        .join(", ")
}

fn field(value: Option<&str>) -> String {
    sanitize(value.unwrap_or("unknown"))
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "es" }
}

fn s(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn paint(text: &str, color: &str, enabled: bool) -> String {
    if enabled {
        format!("{color}{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn bold(text: &str, enabled: bool) -> String {
    paint(text, "\x1b[1m", enabled)
}

pub fn describe_start(started_at_unix: Option<i64>, age_seconds: Option<i64>) -> String {
    match started_at_unix {
        Some(started) => format!(
            "{} UTC ({})",
            format_unix_utc(started),
            age_seconds
                .map(human_age)
                .unwrap_or_else(|| "age unknown".into())
        ),
        None => "unknown".into(),
    }
}

pub fn human_age(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds < 60 {
        return format!("{seconds}s ago");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {}s ago", seconds % 60);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {}m ago", minutes % 60);
    }
    let days = hours / 24;
    format!("{days}d {}h ago", hours % 24)
}

pub fn format_unix_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = of_day / 3_600;
    let minute = (of_day % 3_600) / 60;
    let second = of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Howard Hinnant's civil_from_days, shifted to the Unix epoch.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn listener(pid: u32) -> Listener {
        Listener {
            protocol: "tcp".into(),
            family: Family::Ipv4,
            address: "127.0.0.1".into(),
            port: 3000,
            pid,
            name: Some("node.exe".into()),
            exe: Some(r"C:\Program Files\nodejs\node.exe".into()),
            cmdline: Some(vec!["node".into()]),
            cwd: Some(r"C:\work".into()),
            started_at_unix: Some(1_794_577_643),
            age_seconds: Some(8_070),
            parent: Some(11236),
            children: Vec::new(),
        }
    }

    #[test]
    fn aborted_and_target_exited_json_keep_the_invariants() {
        let target = TargetReport {
            pid: 42,
            started_at_unix: 1_794_577_643,
            other_listening_ports: vec![9000],
        };
        let target_ref = ProcessRef {
            pid: 42,
            started_at_unix: 1_794_577_643,
        };

        let mut aborted = Envelope::new(3000);
        aborted.target = Some(target.clone());
        aborted.kill = KillReport {
            outcome: Outcome::Aborted,
            would_kill: vec![target_ref],
            error: Some(ErrorInfo {
                code: ErrorCode::AccessDenied,
                message: "PID 42 could not be inspected".into(),
            }),
            ..KillReport::default()
        };
        let value = serde_json::to_value(&aborted).unwrap();
        assert!(value["error"].is_null());
        assert_eq!(value["kill"]["error"]["code"], "access_denied");
        assert!(value["kill"]["port_free"].is_null());
        assert!(value["kill"]["port_held_by"].is_null());
        assert!(value["kill"]["respawned"].is_null());
        assert!(value["target"].is_object());

        let mut exited = Envelope::new(3000);
        exited.target = Some(target);
        exited.kill = KillReport {
            outcome: Outcome::TargetExited,
            would_kill: vec![target_ref],
            respawned: Some(Vec::new()),
            port_held_by: Some(Vec::new()),
            port_free: Some(true),
            ..KillReport::default()
        };
        let value = serde_json::to_value(&exited).unwrap();
        assert_eq!(value["kill"]["outcome"], "target_exited");
        assert_eq!(value["kill"]["port_free"], true);
        assert_eq!(value["kill"]["port_held_by"], serde_json::json!([]));
        assert_eq!(value["kill"]["respawned"], serde_json::json!([]));
        assert!(value["kill"]["error"].is_null());
    }

    #[test]
    fn sanitize_strips_c0_except_tab_and_del() {
        assert_eq!(sanitize("a\u{0}b\tc\u{1b}d\u{7f}"), "ab\tcd");
    }

    #[test]
    fn free_port_json_matches_the_spec_shape() {
        let envelope = Envelope::no_listener(3000);
        let value = serde_json::to_value(&envelope).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "schema_version": 1,
                "port": 3000,
                "listeners": [],
                "target": null,
                "kill": {
                    "outcome": "no_listener",
                    "would_kill": [],
                    "killed": [],
                    "survivors": [],
                    "respawned": null,
                    "port_held_by": [],
                    "port_free": true,
                    "error": null
                },
                "warnings": [],
                "error": null
            })
        );
    }

    #[test]
    fn json_keeps_control_characters_raw() {
        let mut l = listener(42);
        l.name = Some("bad\u{1b}[31m.exe".into());
        let envelope = Envelope {
            schema_version: SCHEMA_VERSION,
            port: 3000,
            listeners: vec![l],
            target: None,
            kill: KillReport::default(),
            warnings: vec![Warning {
                code: "stateful_process".into(),
                message: "warn\u{0}".into(),
                pid: 42,
            }],
            error: None,
        };
        let value = serde_json::to_value(&envelope).unwrap();
        assert_eq!(value["listeners"][0]["name"], "bad\u{1b}[31m.exe");
        assert_eq!(value["warnings"][0]["message"], "warn\u{0}");
    }

    #[test]
    fn human_report_groups_rows_by_pid() {
        let mut envelope = Envelope::new(3000);
        envelope.listeners = vec![listener(42), listener(42)];
        envelope.kill.outcome = Outcome::DryRun;
        envelope.kill.would_kill = vec![ProcessRef {
            pid: 42,
            started_at_unix: 1_794_577_643,
        }];

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        write_human(
            &envelope,
            &mut Streams {
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
            ColorConfig::default(),
            false,
        )
        .unwrap();

        let text = String::from_utf8(stdout).unwrap();
        assert_eq!(text.matches("PID 42").count(), 1);
        assert_eq!(text.matches("tcp ipv4 127.0.0.1:3000").count(), 2);
        assert!(text.contains("Dry run: would stop 1 process"));
    }

    #[test]
    fn human_output_strips_controls_from_process_strings() {
        let mut l = listener(42);
        l.name = Some("evil\u{1b}[2J.exe".into());
        l.exe = Some("C:\\x\u{0}\\y.exe".into());
        l.cwd = Some("C:\\work\nnext".into());
        let mut envelope = Envelope::new(3000);
        envelope.listeners = vec![l];
        envelope.warnings = vec![Warning {
            code: "stateful_process".into(),
            message: "warn\u{1b}[31m".into(),
            pid: 42,
        }];
        envelope.error = Some(ErrorInfo::for_code(ErrorCode::AmbiguousPort, 3000, None));

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        write_warnings(&envelope.warnings, &mut stderr, false).unwrap();
        write_human(
            &envelope,
            &mut Streams {
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
            ColorConfig::default(),
            false,
        )
        .unwrap();

        let stdout = String::from_utf8(stdout).unwrap();
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(!stdout.contains('\u{1b}'));
        assert!(!stdout.contains('\u{0}'));
        assert!(!stdout.contains("\nnext"));
        assert!(!stderr.contains('\u{1b}'));
    }

    #[test]
    fn utc_formatting_matches_known_dates() {
        assert_eq!(format_unix_utc(0), "1970-01-01 00:00:00");
        assert_eq!(format_unix_utc(946_684_800), "2000-01-01 00:00:00");
        assert_eq!(format_unix_utc(1_794_577_643), "2026-11-13 13:47:23");
    }

    #[test]
    fn ages_are_human_readable() {
        assert_eq!(human_age(0), "0s ago");
        assert_eq!(human_age(45), "45s ago");
        assert_eq!(human_age(125), "2m 5s ago");
        assert_eq!(human_age(8_070), "2h 14m ago");
    }
}
