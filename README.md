# Portfree

A Windows 10/11 command-line tool that identifies which process owns a TCP port and stops exactly that process after confirmation.

```text
portfree <PORT> [--pid <PID>] [--dry-run] [--json] [--yes] [--no-color]
```

- `--pid` asserts which owner to kill when more than one process listens on the port.
- `--dry-run` resolves and reports, but never kills.
- `--json` emits exactly one JSON document on stdout. By itself it is a dry run; combine it with `--yes` to kill.
- `--yes` skips the prompt. It is the only way to authorize a kill without a TTY.
- `--no-color` (or `NO_COLOR`) disables ANSI color on each stream.

## Build

```powershell
cargo build --release
cargo test
```

The crate is Windows-only and uses the native TCP table (`netstat2`), process metadata (`sysinfo`), and `windows-sys` for handles and safety checks.

## Outcomes

| Outcome | Meaning | Exit |
|---|---|---|
| `no_listener` | Nothing was listening at inspection. | 0 |
| `dry_run` | Target resolved; no kill by design or by declining the prompt. | 0 |
| `not_attempted` | Refused before the kill phase (ambiguous port, system process, no TTY, etc.). | 3 (4 for `no_tty`) |
| `target_exited` | The target exited before the first kill; the port was observed free. | 0 |
| `killed` | The target and descendants were terminated; the port was observed free. | 0 |
| `aborted` | Pre-flight failed after confirmation; nothing was killed. | 3 |
| `failed` | A survivor, respawn, or held port was observed after the kill attempt. | 1 |

Declining the interactive prompt reports `dry_run` and exits 0. EOF counts as "No"; Ctrl+C at the prompt exits 130 with nothing killed.

Pre-flight refusals use `aborted` / `access_denied` (the specific safety reason is carried in the message), or `aborted` / `stale_target` when the target's identity changed after confirmation.

## Tests

End-to-end tests spawn `tests/fixtures/listener.rs` as a separate fixture binary. It can listen normally, dual-stack, with `SO_REUSEADDR`, on extra ports, or wait for a PID to exit and rebind during the observation window. The production binary has no test-only flags.
