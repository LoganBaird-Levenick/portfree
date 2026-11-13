//! Resolution: turn raw table rows and a process snapshot into Listener rows,
//! a Target, and a candidate Kill set. Pure and fully fakeable.

use std::collections::BTreeMap;

use crate::model::{
    ErrorCode, Listener, ProcessInfo, ProcessRef, ProcessSnapshot, RowState, STATEFUL_EXES, TcpRow,
    Warning,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    pub pid: u32,
    pub started_at_unix: Option<i64>,
}

#[derive(Debug)]
pub struct Resolved {
    pub listeners: Vec<Listener>,
    pub target: ProcessRef,
    pub preexisting: Vec<Owner>,
    pub kill_set: Vec<ProcessRef>,
    pub other_listening_ports: Vec<u16>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug)]
pub enum Resolution {
    NoListener,
    Refused {
        code: ErrorCode,
        listeners: Vec<Listener>,
        pid: Option<u32>,
    },
    Resolved(Resolved),
}

pub fn resolve(
    port: u16,
    rows: &[TcpRow],
    snapshot: &ProcessSnapshot,
    requested_pid: Option<u32>,
) -> Resolution {
    let matching: Vec<&TcpRow> = rows
        .iter()
        .filter(|r| r.port == port && r.state == RowState::Listen)
        .collect();

    let listeners_for = |selected: &[u32]| build_listeners(port, &matching, snapshot, selected);

    if matching.is_empty() {
        return match requested_pid {
            Some(pid) => Resolution::Refused {
                code: ErrorCode::PidNotListening,
                listeners: Vec::new(),
                pid: Some(pid),
            },
            None => Resolution::NoListener,
        };
    }

    // An ownerless row is refused before grouping; it cannot be attributed or killed.
    if matching
        .iter()
        .any(|r| r.pids.is_empty() || r.pids.as_slice() == [0])
    {
        let owned: Vec<u32> = distinct_owners(&matching).into_iter().collect();
        return Resolution::Refused {
            code: ErrorCode::PidZero,
            listeners: listeners_for(&owned),
            pid: None,
        };
    }

    let owners = distinct_owners(&matching);
    if owners.is_empty() {
        return Resolution::Refused {
            code: ErrorCode::PidZero,
            listeners: Vec::new(),
            pid: None,
        };
    }

    let target_pid = match requested_pid {
        Some(pid) => {
            if !owners.contains(&pid) {
                return Resolution::Refused {
                    code: ErrorCode::PidNotListening,
                    listeners: listeners_for(&owners),
                    pid: Some(pid),
                };
            }
            pid
        }
        None if owners.len() > 1 => {
            return Resolution::Refused {
                code: ErrorCode::AmbiguousPort,
                listeners: listeners_for(&owners),
                pid: None,
            };
        }
        None => owners[0],
    };

    let listeners = build_listeners(port, &matching, snapshot, &owners);

    let target_info = snapshot.get(target_pid);
    let target = match target_info.and_then(|p| p.started_at_unix) {
        Some(started_at_unix) => ProcessRef {
            pid: target_pid,
            started_at_unix,
        },
        None if target_info.is_some() => {
            return Resolution::Refused {
                code: ErrorCode::NotInspectable,
                listeners,
                pid: Some(target_pid),
            };
        }
        None => {
            return Resolution::Refused {
                code: ErrorCode::StaleTarget,
                listeners,
                pid: Some(target_pid),
            };
        }
    };

    let mut kill_set = vec![target];
    for pid in snapshot.descendants_of(target_pid) {
        let Some(started_at_unix) = snapshot.get(pid).and_then(|p| p.started_at_unix) else {
            return Resolution::Refused {
                code: ErrorCode::NotInspectable,
                listeners,
                pid: Some(target_pid),
            };
        };
        kill_set.push(ProcessRef {
            pid,
            started_at_unix,
        });
    }

    let preexisting: Vec<Owner> = owners
        .iter()
        .map(|&pid| Owner {
            pid,
            started_at_unix: snapshot.get(pid).and_then(|p| p.started_at_unix),
        })
        .collect();

    let other_listening_ports = other_listening_ports(port, target_pid, rows);
    let warnings = warnings_for(&target, target_info);

    Resolution::Resolved(Resolved {
        listeners,
        target,
        preexisting,
        kill_set,
        other_listening_ports,
        warnings,
    })
}

fn distinct_owners(rows: &[&TcpRow]) -> Vec<u32> {
    let mut owners: Vec<u32> = rows
        .iter()
        .flat_map(|r| r.pids.iter().copied())
        .filter(|pid| *pid != 0)
        .collect();
    owners.sort_unstable();
    owners.dedup();
    owners
}

fn build_listeners(
    port: u16,
    matching: &[&TcpRow],
    snapshot: &ProcessSnapshot,
    owners: &[u32],
) -> Vec<Listener> {
    let mut listeners = Vec::new();
    for row in matching {
        for pid in row.pids.iter().copied().filter(|pid| *pid != 0) {
            if !owners.contains(&pid) {
                continue;
            }
            // Windows reports at most one PID per TCP row. If a backend ever
            // supplies several, each (row, pid) pair becomes a Listener so no
            // owner is hidden from the ambiguity check.
            let info = snapshot.get(pid);
            let started_at_unix = info.and_then(|p| p.started_at_unix);
            listeners.push(Listener {
                protocol: "tcp".into(),
                family: row.family,
                address: row.address.to_string(),
                port,
                pid,
                name: info.and_then(|p| p.name.clone()),
                exe: info.and_then(|p| p.exe.clone()),
                cmdline: info.and_then(|p| p.cmdline.clone()),
                cwd: info.and_then(|p| p.cwd.clone()),
                started_at_unix,
                age_seconds: started_at_unix.map(|started| (snapshot.now_unix - started).max(0)),
                parent: info.and_then(|p| p.parent),
                children: snapshot.children_of(pid),
            });
        }
    }
    listeners
}

fn other_listening_ports(port: u16, pid: u32, rows: &[TcpRow]) -> Vec<u16> {
    let mut ports: Vec<u16> = rows
        .iter()
        .filter(|r| r.state == RowState::Listen)
        .filter(|r| r.port != port && r.pids.contains(&pid))
        .map(|r| r.port)
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn warnings_for(target: &ProcessRef, info: Option<&ProcessInfo>) -> Vec<Warning> {
    let Some(name) = info.and_then(|p| p.name.as_deref()) else {
        return Vec::new();
    };
    let folded = name.to_lowercase();
    if STATEFUL_EXES.contains(&folded.as_str()) {
        vec![Warning::stateful(target.pid, name)]
    } else {
        Vec::new()
    }
}

/// A process snapshot that can be built field-by-field in tests.
pub fn snapshot_from(
    entries: impl IntoIterator<Item = ProcessInfo>,
    now_unix: i64,
) -> ProcessSnapshot {
    let mut processes = BTreeMap::new();
    for info in entries {
        processes.insert(info.pid, info);
    }
    ProcessSnapshot {
        processes,
        now_unix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Family;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn listen(port: u16, addr: IpAddr, pids: &[u32]) -> TcpRow {
        TcpRow {
            family: Family::of(addr),
            address: addr,
            port,
            state: RowState::Listen,
            pids: pids.to_vec(),
        }
    }

    fn other(port: u16, pids: &[u32]) -> TcpRow {
        TcpRow {
            family: Family::Ipv4,
            address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port,
            state: RowState::Other,
            pids: pids.to_vec(),
        }
    }

    fn ipv4(pids: &[u32]) -> TcpRow {
        listen(3000, IpAddr::V4(Ipv4Addr::UNSPECIFIED), pids)
    }

    fn proc(pid: u32, parent: Option<u32>, start: Option<i64>, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent,
            name: Some(name.into()),
            exe: Some(format!("C:\\tools\\{name}")),
            cmdline: Some(vec![name.into()]),
            cwd: Some("C:\\work".into()),
            started_at_unix: start,
        }
    }

    fn snapshot(procs: &[ProcessInfo]) -> ProcessSnapshot {
        snapshot_from(procs.iter().cloned(), 1_000)
    }

    #[test]
    fn one_owner_is_the_target_and_each_row_is_preserved() {
        let rows = [
            ipv4(&[42]),
            listen(3000, IpAddr::V6(Ipv6Addr::UNSPECIFIED), &[42]),
        ];
        let snap = snapshot(&[proc(42, Some(1), Some(500), "node.exe")]);

        let Resolution::Resolved(resolved) = resolve(3000, &rows, &snap, None) else {
            panic!("expected a resolution");
        };

        assert_eq!(resolved.target.pid, 42);
        assert_eq!(resolved.target.started_at_unix, 500);
        assert_eq!(resolved.listeners.len(), 2);
        assert_eq!(resolved.listeners[0].family, Family::Ipv4);
        assert_eq!(resolved.listeners[1].family, Family::Ipv6);
        assert_eq!(resolved.listeners[1].address, "::");
        assert_eq!(resolved.listeners[0].age_seconds, Some(500));
        assert_eq!(resolved.kill_set, vec![resolved.target]);
    }

    #[test]
    fn two_owners_are_an_ambiguous_port() {
        let rows = [ipv4(&[42]), ipv4(&[43])];
        let snap = snapshot(&[
            proc(42, Some(1), Some(500), "a.exe"),
            proc(43, Some(1), Some(501), "b.exe"),
        ]);

        let Resolution::Refused {
            code, listeners, ..
        } = resolve(3000, &rows, &snap, None)
        else {
            panic!("expected refusal");
        };
        assert_eq!(code, ErrorCode::AmbiguousPort);
        assert_eq!(listeners.len(), 2);
    }

    #[test]
    fn pid_selects_an_owner() {
        let rows = [ipv4(&[42]), ipv4(&[43])];
        let snap = snapshot(&[
            proc(42, Some(1), Some(500), "a.exe"),
            proc(43, Some(1), Some(501), "b.exe"),
        ]);

        let Resolution::Resolved(resolved) = resolve(3000, &rows, &snap, Some(43)) else {
            panic!("expected a resolution");
        };
        assert_eq!(resolved.target.pid, 43);
    }

    #[test]
    fn pid_must_be_an_owner() {
        let rows = [ipv4(&[42]), ipv4(&[43])];
        let snap = snapshot(&[
            proc(42, Some(1), Some(500), "a.exe"),
            proc(43, Some(1), Some(501), "b.exe"),
        ]);

        let Resolution::Refused { code, .. } = resolve(3000, &rows, &snap, Some(99)) else {
            panic!("expected refusal");
        };
        assert_eq!(code, ErrorCode::PidNotListening);
    }

    #[test]
    fn a_free_port_is_no_listener() {
        let snap = snapshot(&[]);
        assert!(matches!(
            resolve(3000, &[], &snap, None),
            Resolution::NoListener
        ));
    }

    #[test]
    fn a_free_port_asserted_with_pid_is_pid_not_listening() {
        let snap = snapshot(&[]);
        let Resolution::Refused { code, .. } = resolve(3000, &[], &snap, Some(42)) else {
            panic!("expected refusal");
        };
        assert_eq!(code, ErrorCode::PidNotListening);
    }

    #[test]
    fn an_ownerless_row_is_pid_zero_before_grouping() {
        let rows = [ipv4(&[0]), ipv4(&[42])];
        let snap = snapshot(&[proc(42, Some(1), Some(500), "a.exe")]);

        let Resolution::Refused { code, .. } = resolve(3000, &rows, &snap, None) else {
            panic!("expected refusal");
        };
        assert_eq!(code, ErrorCode::PidZero);
    }

    #[test]
    fn empty_associated_pids_is_pid_zero() {
        let rows = [ipv4(&[])];
        let snap = snapshot(&[]);

        let Resolution::Refused { code, .. } = resolve(3000, &rows, &snap, None) else {
            panic!("expected refusal");
        };
        assert_eq!(code, ErrorCode::PidZero);
    }

    #[test]
    fn time_wait_rows_do_not_count_as_listeners() {
        let rows = [other(3000, &[42])];
        let snap = snapshot(&[proc(42, Some(1), Some(500), "a.exe")]);
        assert!(matches!(
            resolve(3000, &rows, &snap, None),
            Resolution::NoListener
        ));
    }

    #[test]
    fn other_listening_ports_are_deduped_sorted_and_exclude_the_query() {
        let rows = [
            ipv4(&[42]),
            listen(9000, IpAddr::V4(Ipv4Addr::LOCALHOST), &[42]),
            listen(500, IpAddr::V4(Ipv4Addr::LOCALHOST), &[42]),
            listen(9000, IpAddr::V6(Ipv6Addr::LOCALHOST), &[42]),
            listen(7000, IpAddr::V4(Ipv4Addr::LOCALHOST), &[43]),
        ];
        let snap = snapshot(&[
            proc(42, Some(1), Some(500), "a.exe"),
            proc(43, Some(1), Some(501), "b.exe"),
        ]);

        let Resolution::Resolved(resolved) = resolve(3000, &rows, &snap, None) else {
            panic!("expected a resolution");
        };
        assert_eq!(resolved.other_listening_ports, vec![500, 9000]);
    }

    #[test]
    fn children_are_direct_but_the_kill_set_is_transitive() {
        let rows = [ipv4(&[10])];
        let snap = snapshot(&[
            proc(1, None, Some(100), "parent.exe"),
            proc(10, Some(1), Some(200), "target.exe"),
            proc(11, Some(10), Some(300), "child.exe"),
            proc(12, Some(11), Some(400), "grandchild.exe"),
        ]);

        let Resolution::Resolved(resolved) = resolve(3000, &rows, &snap, None) else {
            panic!("expected a resolution");
        };
        assert_eq!(resolved.listeners[0].children, vec![11]);
        assert_eq!(
            resolved.kill_set.iter().map(|r| r.pid).collect::<Vec<_>>(),
            vec![10, 11, 12]
        );
        assert!(!resolved.kill_set.iter().any(|r| r.pid == 1));
    }

    #[test]
    fn a_missing_target_is_stale() {
        let rows = [ipv4(&[42])];
        let snap = snapshot(&[]);
        let Resolution::Refused { code, .. } = resolve(3000, &rows, &snap, None) else {
            panic!("expected refusal");
        };
        assert_eq!(code, ErrorCode::StaleTarget);
    }

    #[test]
    fn a_zero_start_time_is_not_inspectable() {
        let rows = [ipv4(&[42])];
        let snap = snapshot(&[proc(42, Some(1), None, "a.exe")]);
        let Resolution::Refused { code, .. } = resolve(3000, &rows, &snap, None) else {
            panic!("expected refusal");
        };
        assert_eq!(code, ErrorCode::NotInspectable);
    }

    #[test]
    fn a_descendant_without_a_start_time_is_not_inspectable() {
        let rows = [ipv4(&[42])];
        let snap = snapshot(&[
            proc(42, Some(1), Some(500), "a.exe"),
            proc(43, Some(42), None, "child.exe"),
        ]);
        let Resolution::Refused { code, .. } = resolve(3000, &rows, &snap, None) else {
            panic!("expected refusal");
        };
        assert_eq!(code, ErrorCode::NotInspectable);
    }

    #[test]
    fn a_stateful_target_gets_an_advisory_warning() {
        let rows = [ipv4(&[42])];
        let snap = snapshot(&[proc(42, Some(1), Some(500), "Postgres.EXE")]);

        let Resolution::Resolved(resolved) = resolve(3000, &rows, &snap, None) else {
            panic!("expected a resolution");
        };
        assert_eq!(resolved.warnings.len(), 1);
        assert_eq!(resolved.warnings[0].code, "stateful_process");
        assert_eq!(resolved.warnings[0].pid, 42);
    }
}
