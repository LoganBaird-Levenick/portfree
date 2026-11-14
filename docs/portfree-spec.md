# Portfree v1 Specification

A Windows CLI that identifies which process owns a TCP port and stops exactly that process after confirmation.

## Platform and dependencies

- Windows 10/11, `x86_64-pc-windows-msvc`, Rust 1.100.
- `netstat2` 0.11.2 (native TCP table), `sysinfo` 0.40.3 (process metadata), `windows-sys` 0.61.2 (handles, safety checks), `clap` 4.6.7, `serde`/`serde_json`.

## Non-goals

UDP. Multiple ports per invocation. A global port listing. Config or state. A non-Windows backend. Raw command line / PEB reading. Graceful shutdown. `--force`. Parent-supervisor heuristics.

Distribution and release tooling are governed by [ADR-0004](adr/0004-public-distribution-and-release-tooling.md); they are not part of the runtime scope.

## CLI

```
portfree <PORT> [--pid <PID>] [--dry-run] [--json] [--yes] [--no-color]
```

- `PORT` is 1–65535; `0` is a usage error. No arguments, or `--pid` without a port, is a usage error (exit 2).
- `--pid` is valid only with a port and only if that PID owns a Listener row on the port; otherwise `pid_not_listening`, exit 3.
- Precedence: `--dry-run` wins over `--yes`; `--yes` wins over `--json`'s implied dry-run. `--json --yes` kills; `--json` alone reports only.
- An interactive kill requires both stdin and stderr to be TTYs. Otherwise exit 4 `no_tty` unless `--yes`, `--dry-run`, or `--json` alone (which is dry-run).
- `--no-color` and `NO_COLOR` disable color per stream; `--json` disables color unconditionally.
- Ctrl+C at the prompt exits 130 with nothing killed. EOF at the prompt is No. SIGINT is blocked during the kill batch.

## Resolution

- Read the native TCP table for IPv4 and IPv6, TCP only; keep rows in state `LISTEN` for the requested port.
- A **Listener** is one row, scoped to one address family. Rows are never merged. A dual-mode socket appears as two Listener rows with one owner.
- Rows with no owner (`associated_pids` empty or `[0]`) are a `pid_zero` refusal, checked before grouping.
- Group Listener rows by owner PID.
  - Exactly one distinct non-zero PID: that process is the **Target**.
  - More than one: `ambiguous_port`, exit 3, unless `--pid` selects one owner.
  - `--pid` must be among the owners; otherwise `pid_not_listening`, exit 3.
- The Target is identified by `(pid, started_at_unix)`. Hold a `PROCESS_QUERY_LIMITED_INFORMATION` handle across the prompt; open `PROCESS_TERMINATE` only after consent. Compare `GetProcessTimes` creation time with the enumeration snapshot; mismatch is `stale_target`; unreadable or zero is `not_inspectable`.
- Refuse if the Target is PID 0, critical (`IsProcessCritical`), in session 0, owned by a service account or another user, under the canonical Windows directory, or the current process/an ancestor.
- The human report groups Listener rows by PID. JSON `listeners` is one row per Listener; consumers group by PID.

## Safety checks

Checked before the prompt, and again for every kill-set member during pre-flight:

- PID 0 or no owner.
- `IsProcessCritical`.
- Session 0 (`ProcessIdToSessionId`).
- Token SID: `SYSTEM`, `LOCAL SERVICE`, or `NETWORK SERVICE` (`service_account`), or a user other than the current user (`other_user`).
- Executable path under the canonical Windows directory: `GetSystemWindowsDirectoryW`, `GetLongPathNameW`, strip `\\?\`, case-fold, compare component-wise. `C:\Windows10Upgrade` does not match `C:\Windows`.
- Current process or an ancestor of it.
- Unreadable executable path or creation time (`not_inspectable`).

## Kill and observation

- The prompt's descendant count is from inspection. After confirmation, re-snapshot the Target's descendants (direct and indirect), kill any new child, and report the actual set without re-prompting.
- Pre-flight every handle before the first kill. Apply the same safety checks to every kill-set member. A descendant that no longer exists is dropped; an access failure aborts with `aborted` / `access_denied` and kills nothing.
- Kill set = Target + descendants. Ancestors are never included.
- Kill the Target first because it holds the socket; then its descendants. Forced `TerminateProcess`; no graceful phase.
- Wait on the held handles (2 s) for survivors, then observe the port for 1 s, polling about every 50 ms. The window runs after every successful kill and after `target_exited`; `no_listener` does not run it.
- Respawn is a new `(pid, started_at_unix)` not present in the pre-kill snapshot, in `LISTEN`, ignoring PID 0. Survivors preempt respawn classification.
- `killed` and `target_exited` require the port to be observed free. A co-listener or new holder is `failed` with `port_still_held` or `respawned`.
- `stateful_process` warns (advisory) for `postgres.exe`, `mysqld.exe`, `sqlservr.exe`, `mongod.exe`, `redis-server.exe`, and `com.docker.backend.exe`, matched case-insensitively. No parent-supervisor warning.

## Streams

- Human mode: the report and final result go to stdout; the prompt and diagnostics go to stderr. The prompt repeats PID, exe, cwd, start time, and descendant count.
- `--json`: exactly one JSON document on stdout from the moment parsing succeeds. Human diagnostics may go to stderr, TTY-only. Envelope errors stay on stdout and are not duplicated on stderr. Clap/parse errors are the exception: exit 2, stderr, no JSON.
- Sanitize C0 (except tab) and DEL in every process-derived string at the human-output boundary, including `warnings[].message` and `error.message`. JSON keeps the raw string; `serde_json` escapes it.

## JSON envelope

```json
{
  "schema_version": 1,
  "port": 3000,
  "listeners": [
    {
      "protocol": "tcp",
      "family": "ipv4",
      "address": "0.0.0.0",
      "port": 3000,
      "pid": 18412,
      "name": "node.exe",
      "exe": "C:\\Program Files\\nodejs\\node.exe",
      "cmdline": ["node", "...\\vite.js"],
      "cwd": "C:\\Users\\Logan\\src\\shop\\web",
      "started_at_unix": 1794577643,
      "age_seconds": 8070,
      "parent": 11236,
      "children": [18413]
    }
  ],
  "target": {
    "pid": 18412,
    "started_at_unix": 1794577643,
    "other_listening_ports": [24678]
  },
  "kill": {
    "outcome": "killed",
    "would_kill": [{"pid": 18412, "started_at_unix": 1794577643}],
    "killed": [{"pid": 18412, "started_at_unix": 1794577643}],
    "survivors": [],
    "respawned": [],
    "port_held_by": [],
    "port_free": true,
    "error": null
  },
  "warnings": [],
  "error": null
}
```

- `listeners` is one row per Listener. `children` is direct children only; process metadata repeats on each Listener row for the same owner. The full kill set is in `would_kill`.
- `target` carries identity plus `other_listening_ports`; full details stay in `listeners`.
- `would_kill`, `killed`, `survivors`, and `port_held_by` entries are `{pid, started_at_unix}`. `port_held_by` lists only pre-existing owners not in the kill set; new owners go in `respawned`. On `failed` / `port_still_held` it explains the holder; on `failed` / `respawned` the new holder is in `respawned`, and any co-listener remains in `port_held_by`.
- `respawned` entries are `{pid, started_at_unix, name, exe, cmdline}`.
- `warnings` entries are `{code, message, pid}`.
- `error` and `kill.error` are `{code, message}` or `null`.
- `cmdline` is `string[] | null`: best-effort tokenization, display-only, do not re-execute. `null` is unreadable; `[]` is genuinely empty.
- `started_at_unix`, `age_seconds`, `exe`, `cwd`, and `cmdline` are `null` when unreadable, never omitted or zero. `age_seconds` is derived; consumers compute from `started_at_unix`.

A free port:

```json
{
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
}
```

## Outcomes and exit codes

| `outcome` | Meaning | Exit |
|---|---|---|
| `no_listener` | Nothing listening at inspection; no kill phase. | 0 |
| `dry_run` | Target resolved, no kill by design. | 0 |
| `not_attempted` | Refused before the kill phase: ambiguous, system process, `--pid` mismatch, query failure, no TTY. | 3 (4 for `no_tty`) |
| `target_exited` | Target gone before the first kill; port observed free. | 0 |
| `killed` | All targets terminated; port observed free. | 0 |
| `aborted` | Pre-flight failed after confirmation; nothing killed. | 3 |
| `failed` | Kill attempted; survivors, respawn, or port still held. | 1 |

`--pid` on a free port is `not_attempted` / `pid_not_listening`, not `no_listener`: the assertion failed.

## Error codes

Only `code` is stable; `message` is human text and may change between patch versions. Unknown codes fall back to `outcome` and the exit code.

| Code | Outcome | Exit |
|---|---|---|
| `ambiguous_port` | `not_attempted` | 3 |
| `pid_not_listening` | `not_attempted` | 3 |
| `pid_zero` | `not_attempted` | 3 |
| `critical_process` | `not_attempted` | 3 |
| `session_zero` | `not_attempted` | 3 |
| `service_account` | `not_attempted` | 3 |
| `other_user` | `not_attempted` | 3 |
| `windows_directory` | `not_attempted` | 3 |
| `ancestor_process` | `not_attempted` | 3 |
| `not_inspectable` | `not_attempted` | 3 |
| `query_failed` | `not_attempted` | 3 |
| `no_tty` | `not_attempted` | 4 |
| `stale_target` | `not_attempted` or `aborted` | 3 |
| `access_denied` | `aborted` | 3 |
| `survivor` | `failed` | 1 |
| `respawned` | `failed` | 1 |
| `port_still_held` | `failed` | 1 |
| `kill_failed` | `failed` | 1 |

## Invariants

1. `would_kill`, `killed`, and `survivors` entries always have non-null `started_at_unix`; `not_inspectable` exists so this holds.
2. `listeners` is authoritative only when `error == null`. On `query_failed`, `[]` means unknown, not none.
3. `other_listening_ports` is deduped, sorted, and excludes the queried port.
4. `target` is null for `no_listener` and every `not_attempted` outcome, including `pid_zero`, `ambiguous_port`, and `not_inspectable`.
5. `error` is non-null iff `outcome == "not_attempted"`; `kill.error` is non-null iff `outcome` is `aborted` or `failed`; never both.
6. `killed` and `target_exited` imply `port_free: true` and `port_held_by: []`. Only `failed` can have `port_free: false`.
7. `respawned`: `[]` means checked and none; `null` means not observed.
8. `port_free`: `null` means not observed; `no_listener` is true at inspection; `dry_run`, `not_attempted`, and `aborted` are null.
9. `port_held_by`: `[]` for `no_listener`, `killed`, and `target_exited`; pre-existing co-listeners still listening for `dry_run` (prediction) and `failed`; `null` for `not_attempted` and `aborted`. New owners are always in `respawned`, never in `port_held_by`.
10. `cmdline` is display-only and not reversible; `null` is unreadable, `[]` is empty.
11. `age_seconds` is derived; consumers compute from `started_at_unix`.
12. `listeners` rows are not owners; group by PID.
13. Additive-only after v1; breaking changes bump `schema_version`.

## Test obligations

- Spawn a test listener binary on an ephemeral port; assert `portfree` reports its PID and the JSON shape, and exits 0.
- Dual-mode socket: two Listener rows, one Target; human output groups by PID.
- `SO_REUSEADDR` two-process hijack: `ambiguous_port`; `--pid` kills one and reports `failed` / `port_still_held`.
- `--pid` on a free port: `pid_not_listening`, exit 3.
- No listener: `no_listener`, exit 0.
- System refusal: PID 4 / critical process (or session 0 on hosts that do not report PID 4 as critical).
- Fake-table tests: `pid_zero`, `ambiguous_port`, `stale_target`, `not_inspectable`.
- TTY gate: stdin or stderr not a TTY yields `no_tty`, exit 4; `echo y | portfree 3000` does not authorize.
- JSON snapshot per outcome; unknown-code tolerance.
- sysinfo `start_time` and `GetProcessTimes` agree under the same truncation.
- C0 sanitization in human output; JSON remains raw.
- `other_listening_ports` dedupe, sort, exclude.
- Respawn: helper rebinds within the observation window, yielding `failed` / `respawned`.
- TIME_WAIT connections to the killed port do not count as listeners.

The test listener is a separate fixture binary. No test-only flags in the production `portfree` binary.
