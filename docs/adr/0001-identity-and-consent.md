# Identity and consent: hold a query handle, open kill rights only after confirmation

A PID is not an identity on Windows: PIDs are recycled, and the confirmation prompt is a window in which the target can exit and its PID be reused. `portfree` therefore opens a `PROCESS_QUERY_LIMITED_INFORMATION` handle when it resolves a target, holds that handle across the prompt, and verifies `GetProcessTimes` creation time against the enumeration snapshot. Only after the user confirms does it open `PROCESS_TERMINATE` and kill through the handle. Holding the query handle keeps the process object, and therefore the PID, reserved for the entire decision window, so the post-consent `OpenProcess` cannot resolve to a different process. It also keeps kill rights out of the tool's hands while the user is deciding. The creation-time comparison remains as a belt-and-braces check for the gap between the TCP table read and the first handle open.

## Considered Options

- **PID plus a creation-time comparison immediately before `TerminateProcess`.** Rejected: compare-then-kill is still TOCTOU, and sysinfo's `start_time` is whole seconds.
- **Hold a `PROCESS_TERMINATE` handle across the prompt.** Rejected: it closes the reuse window but holds kill rights during the decision.
- **Hold a query-only handle, open `PROCESS_TERMINATE` after consent.** Chosen: it closes the window and separates consent from capability.

## Consequences

- `stale_target` is still possible between the TCP table read and the first handle open.
- If the target exits during the prompt, `WaitForSingleObject(handle, 0)` reports it before `PROCESS_TERMINATE` is opened; the outcome is `target_exited`, not `access_denied`.
- A test must assert that sysinfo's `start_time` and `GetProcessTimes` agree under the same truncation, so a sysinfo upgrade cannot silently disable the check.
- Descendants are snapshotted after confirmation and pre-flighted before the first kill; the target's held query handle protects only the target.
