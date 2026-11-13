# Refuse rather than guess, and never attempt a graceful shutdown

When multiple processes own listeners on the same port, or when the target is a system process, `portfree` refuses instead of picking one. The only override is an explicit `--pid`, which asserts which owner the user means; it removes no safety check. There is no `--force`. The same principle applies to shutdown semantics: the tool force-terminates rather than guessing at `WM_CLOSE` or `CTRL_BREAK`, because those mechanisms depend on the target having a window or a console and are a per-application lottery, not a policy.

## Consequences

- Ambiguity is `ambiguous_port`, exit 3; the tool never kills the first match.
- A `--pid` kill that leaves a co-listener is `failed` / `port_still_held`, not `respawned`.
- System-process refusal is five checks: `IsProcessCritical`, session 0, token SID (service accounts and `other_user`), canonicalized Windows-directory path, and self/ancestors.
- Forced termination can lose unflushed data. The advisory `stateful_process` warning is the only mitigation; the observation window detects a respawn but cannot prevent it.
