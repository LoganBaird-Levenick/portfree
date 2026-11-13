# Read the native Windows TCP table instead of shelling out

`portfree` enumerates listeners with `GetExtendedTcpTable` through the `netstat2` crate. Spawning PowerShell or parsing `netstat` adds a process, a latency floor, and a PowerShell dependency; `netstat` output is localized and text-shaped, so parsing it breaks on non-English Windows. The native table returns structured address, port, state, and owning PID in one call. `Get-NetTCPConnection` and `MSFT_NetTCPConnection` model a dual-mode socket once, while the underlying address-family table, and therefore netstat2, reports it once per family; that row-model difference is recorded as an observation, not the reason for the choice. The JSON deliberately exposes the raw rows.

## Consequences

- Windows-only by construction. A Linux backend attaches at the `PortTable`/`ProcessTable` seam; no Windows types cross it.
- `associated_pids` may be empty or `[0]` for ownerless rows; both are treated as "no killable owner" and refused as `pid_zero`.
- No localized text is parsed anywhere.
