# Portfree

A local Windows command-line tool that identifies which process holds a TCP port and stops exactly that process on request.

## Language

**Listener**:
A single `LISTEN` row in the native Windows TCP table, scoped to one address family. A dual-mode IPv6 socket (`IPV6_V6ONLY=0`) is one kernel socket that appears as two Listener rows; the table cannot distinguish that from two family-specific binds by the same process. Listener rows are never merged. `family` describes the row, not necessarily how the socket was bound.
_Avoid_: Connection, socket, binding, server

**Port owner**:
A process that owns at least one Listener row on a port. A port can have more than one owner.
_Avoid_: Holder, server, service

**Target**:
The process selected for termination, identified by `(pid, started_at_unix)`. Resolution groups Listener rows by owner PID; multiple rows with one PID are one Target.
_Avoid_: Process, victim, socket

**Co-listener**:
A port owner other than the Target that owns a Listener row on the same port. It is not part of the Kill set.
_Avoid_: Sibling, secondary owner

**Kill set**:
The Target and its descendants at the moment of confirmation. Ancestors are never part of it.
_Avoid_: Process tree, process group

**Ambiguous port**:
A port with more than one Port owner, where choosing a Target would require guessing.
_Avoid_: Shared port, conflict

**System process**:
A process outside the tool's kill authority because it is critical to Windows, belongs to another user or session, or is the tool's own ancestor.
_Avoid_: Protected process

**Observation window**:
The fixed one-second period after a kill, or after `target_exited`, during which the port is checked for respawn.
_Avoid_: Grace period, timeout

**Respawn**:
A Listener row that appears on the port during the Observation window and was not present in the pre-kill snapshot.
_Avoid_: Restart, relaunch
