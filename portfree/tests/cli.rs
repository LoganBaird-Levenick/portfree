//! End-to-end tests through the public CLI boundary, using the fixture listener.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use serde_json::Value;

const DETACHED_PROCESS: u32 = 0x0000_0008;
const LISTENER: &str = env!("CARGO_BIN_EXE_portfree-test-listener");
const PORTFREE: &str = env!("CARGO_BIN_EXE_portfree");

fn command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.creation_flags(DETACHED_PROCESS);
    command
}

#[derive(Default, Clone, Copy)]
struct ListenOptions {
    port: Option<u16>,
    reuse: bool,
    dual: bool,
    extra: usize,
}

struct Fixture {
    child: Child,
    port: u16,
    extra_ports: Vec<u16>,
}

impl Fixture {
    fn spawn(options: ListenOptions) -> Self {
        let mut args: Vec<String> = Vec::new();
        if let Some(port) = options.port {
            args.push("--port".into());
            args.push(port.to_string());
        }
        if options.reuse {
            args.push("--reuse".into());
        }
        if options.dual {
            args.push("--dual".into());
        }
        if options.extra > 0 {
            args.push("--extra".into());
            args.push(options.extra.to_string());
        }

        let mut child = command(LISTENER)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn fixture listener");
        let stdout = child.stdout.take().expect("fixture stdout");
        let mut reader = BufReader::new(stdout);

        let mut port = None;
        let mut extra_ports = Vec::new();
        while port.is_none() || extra_ports.len() < options.extra {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read fixture port line");
            assert!(
                !line.is_empty(),
                "fixture closed stdout before printing its port"
            );
            if let Some(value) = line.trim().strip_prefix("PORT=") {
                port = Some(value.parse().expect("fixture port"));
            } else if let Some(value) = line.trim().strip_prefix("EXTRA_PORT=") {
                extra_ports.push(value.parse().expect("extra port"));
            }
        }

        Self {
            child,
            port: port.expect("port"),
            extra_ports,
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn is_alive(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if self.child.try_wait().ok().flatten().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct RespawnHelper {
    child: Child,
    stdout: BufReader<std::process::ChildStdout>,
}

impl RespawnHelper {
    fn spawn(port: u16, after_pid: u32) -> Self {
        let mut child = command(LISTENER)
            .args([
                "--mode",
                "respawn",
                "--port",
                &port.to_string(),
                "--after-pid",
                &after_pid.to_string(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn respawn helper");
        let stdout = BufReader::new(child.stdout.take().expect("respawn stdout"));
        Self { child, stdout }
    }

    fn wait_until_bound(&mut self) -> u16 {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read respawn line");
        line.trim()
            .strip_prefix("RESPAWNED=")
            .expect("RESPAWNED line")
            .parse()
            .expect("respawned port")
    }
}

impl Drop for RespawnHelper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct TimeWaitFixture {
    child: Child,
    port: u16,
}

impl TimeWaitFixture {
    fn spawn() -> Self {
        let mut child = command(LISTENER)
            .args(["--mode", "time-wait", "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn time-wait fixture");
        let mut stdout = BufReader::new(child.stdout.take().expect("time-wait stdout"));

        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .expect("read time-wait port line");
        let port: u16 = line
            .trim()
            .strip_prefix("PORT=")
            .expect("PORT line")
            .parse()
            .expect("time-wait port");

        let client = TcpStream::connect(("127.0.0.1", port)).expect("connect time-wait fixture");

        line.clear();
        stdout
            .read_line(&mut line)
            .expect("read time-wait CLOSED line");
        assert_eq!(line.trim(), "CLOSED=1");

        drop(client);

        line.clear();
        stdout
            .read_line(&mut line)
            .expect("read time-wait DONE line");
        assert_eq!(line.trim(), "DONE=1");

        Self { child, port }
    }

    fn assert_server_side_time_wait(&self) {
        use netstat2::{
            AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState, get_sockets_info,
        };

        let sockets =
            get_sockets_info(AddressFamilyFlags::IPV4, ProtocolFlags::TCP).expect("read TCP table");
        let rows: Vec<_> = sockets
            .iter()
            .filter_map(|socket| match &socket.protocol_socket_info {
                ProtocolSocketInfo::Tcp(tcp) if tcp.local_port == self.port => Some(tcp),
                _ => None,
            })
            .collect();

        assert!(
            !rows.iter().any(|row| row.state == TcpState::Listen),
            "fixture listener should be closed: {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.state == TcpState::TimeWait),
            "expected a server-side TIME_WAIT row on port {}: {rows:?}",
            self.port
        );
    }
}

impl Drop for TimeWaitFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Run {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Run {
    fn code(&self) -> i32 {
        self.status.code().expect("exit code")
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "stdout was not one JSON document ({e}):\nstdout:\n{}\nstderr:\n{}",
                self.stdout, self.stderr
            )
        })
    }
}

fn run_portfree(args: &[&str], stdin: Option<&str>) -> Run {
    let mut command = command(PORTFREE);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = command.spawn().expect("spawn portfree");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut child_stdout = child.stdout.take().expect("stdout");
    let mut child_stderr = child.stderr.take().expect("stderr");
    child_stdout
        .read_to_string(&mut stdout)
        .expect("read stdout");
    child_stderr
        .read_to_string(&mut stderr)
        .expect("read stderr");
    let status = child.wait().expect("wait portfree");
    Run {
        status,
        stdout,
        stderr,
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral")
        .local_addr()
        .expect("local addr")
        .port()
}

fn assert_invariants(value: &Value) {
    let outcome = value["kill"]["outcome"].as_str().unwrap();
    let error = &value["error"];
    let kill_error = &value["kill"]["error"];

    if outcome == "not_attempted" {
        assert!(!error.is_null(), "not_attempted must carry error");
        assert!(
            value["target"].is_null(),
            "not_attempted target must be null"
        );
    } else {
        assert!(error.is_null(), "non-not_attempted must not carry error");
    }

    match outcome {
        "aborted" | "failed" => assert!(!kill_error.is_null(), "{outcome} must carry kill.error"),
        _ => assert!(kill_error.is_null(), "{outcome} must not carry kill.error"),
    }

    if outcome == "not_attempted" || outcome == "aborted" {
        assert!(value["kill"]["port_free"].is_null());
    }
    if outcome == "killed" || outcome == "target_exited" {
        assert_eq!(value["kill"]["port_free"], true);
        assert_eq!(value["kill"]["port_held_by"], serde_json::json!([]));
    }
}

#[test]
fn no_arguments_is_a_usage_error_without_json() {
    let run = run_portfree(&[], None);
    assert_eq!(run.code(), 2);
    assert!(run.stdout.is_empty());
    assert!(run.stderr.contains("PORT"));
}

#[test]
fn port_zero_is_a_usage_error() {
    let run = run_portfree(&["0"], None);
    assert_eq!(run.code(), 2);
    assert!(run.stdout.is_empty());
}

#[test]
fn a_free_port_is_no_listener() {
    let port = free_port();
    let run = run_portfree(&[&port.to_string(), "--json"], None);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr);
    let value = run.json();
    assert_eq!(value["port"], port);
    assert_eq!(value["listeners"], serde_json::json!([]));
    assert_eq!(value["target"], Value::Null);
    assert_eq!(value["kill"]["outcome"], "no_listener");
    assert_eq!(value["kill"]["port_free"], true);
    assert_eq!(value["kill"]["respawned"], Value::Null);
    assert_invariants(&value);
}

#[test]
fn a_free_port_asserted_with_pid_is_pid_not_listening() {
    let port = free_port();
    let run = run_portfree(&[&port.to_string(), "--pid", "424242", "--json"], None);
    assert_eq!(run.code(), 3);
    let value = run.json();
    assert_eq!(value["kill"]["outcome"], "not_attempted");
    assert_eq!(value["error"]["code"], "pid_not_listening");
    assert!(value["target"].is_null());
    assert_invariants(&value);
}

#[test]
fn a_listener_is_reported_by_pid_with_the_expected_json_shape() {
    let mut fixture = Fixture::spawn(ListenOptions::default());
    let port = fixture.port.to_string();
    let run = run_portfree(&[&port, "--json"], None);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr);

    let value = run.json();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["port"], fixture.port);
    assert_eq!(value["listeners"].as_array().unwrap().len(), 1);

    let listener = &value["listeners"][0];
    assert_eq!(listener["protocol"], "tcp");
    assert_eq!(listener["family"], "ipv4");
    assert_eq!(listener["pid"], fixture.pid());
    assert_eq!(listener["port"], fixture.port);
    assert!(listener["started_at_unix"].is_number());
    assert!(listener["age_seconds"].is_number());
    assert!(
        listener["name"]
            .as_str()
            .unwrap()
            .contains("portfree-test-listener")
    );
    assert!(
        listener["exe"]
            .as_str()
            .unwrap()
            .contains("portfree-test-listener")
    );
    assert!(listener["cwd"].is_string());
    assert!(listener["cmdline"].is_array());
    assert!(listener["children"].is_array());

    assert_eq!(value["target"]["pid"], fixture.pid());
    assert_eq!(
        value["target"]["started_at_unix"],
        listener["started_at_unix"]
    );
    assert_eq!(value["kill"]["outcome"], "dry_run");
    assert_eq!(value["kill"]["would_kill"][0]["pid"], fixture.pid());
    assert_eq!(value["kill"]["killed"], serde_json::json!([]));
    assert_eq!(value["kill"]["respawned"], Value::Null);
    assert_eq!(value["kill"]["port_free"], Value::Null);
    assert_invariants(&value);

    assert!(fixture.is_alive(), "--json alone must not kill");
}

#[test]
fn dual_mode_reports_two_listener_rows_with_one_target() {
    let fixture = Fixture::spawn(ListenOptions {
        dual: true,
        ..ListenOptions::default()
    });
    let port = fixture.port.to_string();
    let run = run_portfree(&[&port, "--json"], None);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr);

    let value = run.json();
    let listeners = value["listeners"].as_array().unwrap();
    assert_eq!(
        listeners.len(),
        2,
        "dual-mode socket should be two rows: {value}"
    );
    assert_eq!(listeners[0]["pid"], fixture.pid());
    assert_eq!(listeners[1]["pid"], fixture.pid());
    let families: Vec<&str> = listeners
        .iter()
        .map(|l| l["family"].as_str().unwrap())
        .collect();
    assert!(families.contains(&"ipv4"));
    assert!(families.contains(&"ipv6"));
    assert_eq!(value["target"]["pid"], fixture.pid());

    let human = run_portfree(&[&port, "--dry-run"], None);
    assert_eq!(human.code(), 0, "stderr: {}", human.stderr);
    let text = human.stdout;
    assert_eq!(text.matches(&format!("PID {}", fixture.pid())).count(), 1);
    assert_eq!(text.matches("listeners:").count(), 1);
    assert_eq!(text.matches("tcp ipv4").count(), 1);
    assert_eq!(text.matches("tcp ipv6").count(), 1);
}

#[test]
fn other_listening_ports_are_deduped_sorted_and_exclude_the_query() {
    let fixture = Fixture::spawn(ListenOptions {
        extra: 2,
        ..ListenOptions::default()
    });
    let port = fixture.port.to_string();
    let run = run_portfree(&[&port, "--json"], None);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr);
    let value = run.json();

    let mut expected = fixture.extra_ports.clone();
    expected.sort_unstable();
    let actual: Vec<u16> = value["target"]["other_listening_ports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u16)
        .collect();
    assert_eq!(actual, expected);
    assert!(!actual.contains(&fixture.port));
}

#[test]
fn the_tty_gate_refuses_an_unconfirmed_kill() {
    let mut fixture = Fixture::spawn(ListenOptions::default());
    let port = fixture.port.to_string();

    let piped_yes = run_portfree(&[&port], Some("y\n"));
    assert_eq!(piped_yes.code(), 4, "echo y must not authorize a kill");
    assert!(piped_yes.stdout.contains(&format!("PID {}", fixture.pid())));
    assert!(piped_yes.stderr.contains("no_tty") || piped_yes.stderr.contains("interactive"));
    assert!(fixture.is_alive());

    let no_stdin = run_portfree(&[&port], None);
    assert_eq!(no_stdin.code(), 4);
    assert!(fixture.is_alive());
}

#[test]
fn json_yes_kills_the_listener_and_reports_the_port_free() {
    let mut fixture = Fixture::spawn(ListenOptions::default());
    let port = fixture.port.to_string();
    let run = run_portfree(&[&port, "--json", "--yes"], None);
    assert_eq!(
        run.code(),
        0,
        "stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );

    let value = run.json();
    assert_eq!(value["kill"]["outcome"], "killed");
    assert_eq!(value["kill"]["killed"][0]["pid"], fixture.pid());
    assert_eq!(value["kill"]["survivors"], serde_json::json!([]));
    assert_eq!(value["kill"]["respawned"], serde_json::json!([]));
    assert_eq!(value["kill"]["port_held_by"], serde_json::json!([]));
    assert_eq!(value["kill"]["port_free"], true);
    assert_invariants(&value);

    assert!(
        fixture.wait_for_exit(Duration::from_secs(3)),
        "fixture should be gone"
    );
}

#[test]
fn yes_kills_in_human_mode_without_a_tty() {
    let mut fixture = Fixture::spawn(ListenOptions::default());
    let port = fixture.port.to_string();
    let run = run_portfree(&[&port, "--yes"], None);
    assert_eq!(
        run.code(),
        0,
        "stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    assert!(run.stdout.contains("Stopped 1 process"));
    assert!(run.stdout.contains(&format!("Port {port} is free")));
    assert!(fixture.wait_for_exit(Duration::from_secs(3)));
}

#[test]
fn dry_run_wins_over_yes() {
    let mut fixture = Fixture::spawn(ListenOptions::default());
    let port = fixture.port.to_string();
    let run = run_portfree(&[&port, "--json", "--yes", "--dry-run"], None);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr);
    let value = run.json();
    assert_eq!(value["kill"]["outcome"], "dry_run");
    assert!(fixture.is_alive());
}

#[test]
fn a_co_listener_can_be_selected_with_pid_and_leaves_the_port_held() {
    let mut first = Fixture::spawn(ListenOptions {
        reuse: true,
        ..ListenOptions::default()
    });
    let port = first.port;
    let mut second = Fixture::spawn(ListenOptions {
        port: Some(port),
        reuse: true,
        ..ListenOptions::default()
    });
    let port_s = port.to_string();

    let ambiguous = run_portfree(&[&port_s, "--json"], None);
    assert_eq!(ambiguous.code(), 3, "stderr: {}", ambiguous.stderr);
    let value = ambiguous.json();
    assert_eq!(value["kill"]["outcome"], "not_attempted");
    assert_eq!(value["error"]["code"], "ambiguous_port");
    assert_eq!(value["listeners"].as_array().unwrap().len(), 2);
    assert!(value["target"].is_null());
    assert_invariants(&value);

    let prediction = run_portfree(
        &[&port_s, "--json", "--pid", &first.pid().to_string()],
        None,
    );
    assert_eq!(
        prediction.code(),
        0,
        "stdout: {}\nstderr: {}",
        prediction.stdout,
        prediction.stderr
    );
    let value = prediction.json();
    assert_eq!(value["kill"]["outcome"], "dry_run");
    assert_eq!(value["kill"]["would_kill"][0]["pid"], first.pid());
    assert_eq!(value["kill"]["port_held_by"][0]["pid"], second.pid());
    assert_eq!(value["kill"]["port_free"], Value::Null);
    assert_invariants(&value);

    let selected = run_portfree(
        &[
            &port_s,
            "--json",
            "--yes",
            "--pid",
            &first.pid().to_string(),
        ],
        None,
    );
    assert_eq!(
        selected.code(),
        1,
        "stdout: {}\nstderr: {}",
        selected.stdout,
        selected.stderr
    );
    let value = selected.json();
    assert_eq!(value["kill"]["outcome"], "failed");
    assert_eq!(value["kill"]["error"]["code"], "port_still_held");
    assert_eq!(value["kill"]["killed"][0]["pid"], first.pid());
    assert_eq!(value["kill"]["port_held_by"][0]["pid"], second.pid());
    assert_eq!(value["kill"]["port_free"], false);
    assert_invariants(&value);
    assert!(first.wait_for_exit(Duration::from_secs(3)));
    assert!(second.is_alive());
}

#[test]
fn pid_must_belong_to_a_listener_on_the_port() {
    let fixture = Fixture::spawn(ListenOptions::default());
    let port = fixture.port.to_string();
    let run = run_portfree(&[&port, "--json", "--pid", "424242"], None);
    assert_eq!(
        run.code(),
        3,
        "stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    let value = run.json();
    assert_eq!(value["kill"]["outcome"], "not_attempted");
    assert_eq!(value["error"]["code"], "pid_not_listening");
    assert!(value["target"].is_null());
    assert_invariants(&value);
}

#[test]
fn human_no_listener_says_the_port_is_free() {
    let port = free_port();
    let run = run_portfree(&[&port.to_string(), "--dry-run"], None);
    assert_eq!(run.code(), 0);
    assert!(run.stdout.contains(&format!("Port {port} is free.")));
}

#[test]
fn a_respawn_during_the_observation_window_is_failed() {
    let mut target = Fixture::spawn(ListenOptions::default());
    let port = target.port;
    let mut respawn = RespawnHelper::spawn(port, target.pid());
    let port_s = port.to_string();

    let run = run_portfree(&[&port_s, "--json", "--yes"], None);
    assert_eq!(
        run.code(),
        1,
        "stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    let value = run.json();
    assert_eq!(value["kill"]["outcome"], "failed");
    assert_eq!(value["kill"]["error"]["code"], "respawned");
    assert_eq!(value["kill"]["respawned"][0]["pid"], respawn.child.id());
    assert_eq!(value["kill"]["port_free"], false);
    assert_invariants(&value);

    let rebound = respawn.wait_until_bound();
    assert_eq!(rebound, port);
    assert!(target.wait_for_exit(Duration::from_secs(3)));
}

#[test]
fn time_wait_connections_do_not_count_as_listeners() {
    let fixture = TimeWaitFixture::spawn();
    fixture.assert_server_side_time_wait();

    let run = run_portfree(&[&fixture.port.to_string(), "--json"], None);
    assert_eq!(
        run.code(),
        0,
        "stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    let value = run.json();
    assert_eq!(value["kill"]["outcome"], "no_listener");
    assert_eq!(value["listeners"], serde_json::json!([]));
}

#[test]
fn human_dry_run_reports_the_listener_and_does_not_kill() {
    let mut fixture = Fixture::spawn(ListenOptions::default());
    let port = fixture.port.to_string();
    let run = run_portfree(&[&port, "--dry-run"], None);
    assert_eq!(run.code(), 0, "stderr: {}", run.stderr);
    assert!(
        run.stdout
            .contains(&format!("Port {port} is held by 1 process"))
    );
    assert!(run.stdout.contains(&format!("PID {}", fixture.pid())));
    assert!(
        run.stdout.contains("Dry run: would stop 1 process"),
        "stdout:\n{}\nstderr:\n{}",
        run.stdout,
        run.stderr
    );
    assert!(run.stdout.contains("not touched"));
    assert!(fixture.is_alive());
}
