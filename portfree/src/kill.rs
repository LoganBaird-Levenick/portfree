//! Kill set and observation window.
//!
//! Pre-flight opens every terminate handle and re-runs safety checks before the
//! first kill. The target is killed first because it owns the listening socket.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use crate::clock::Clock;
use crate::model::{
    ErrorCode, ErrorInfo, KillReport, Outcome, ProcessRef, ProcessSnapshot, Respawned, RowState,
};
use crate::ports::PortTable;
use crate::processes::ProcessTable;
use crate::resolve::Owner;
use crate::system::{InspectError, ProcessSystem, QueryToken, TermToken};

pub const SURVIVOR_WAIT_MS: u64 = 2_000;
pub const OBSERVE_MS: u64 = 1_000;
pub const OBSERVE_POLL_MS: u64 = 50;
const WAIT_POLL_MS: u64 = 20;

pub struct KillInput {
    pub port: u16,
    pub target: ProcessRef,
    /// Target first, then descendants, in breadth-first order.
    pub kill_set: Vec<ProcessRef>,
    pub preexisting: Vec<Owner>,
    pub target_query: QueryToken,
    pub snapshot: ProcessSnapshot,
}

pub struct KillDeps<'a> {
    pub ports: &'a mut dyn PortTable,
    pub processes: &'a mut dyn ProcessTable,
    pub system: &'a mut dyn ProcessSystem,
    pub clock: &'a dyn Clock,
}

pub fn execute(input: KillInput, deps: &mut KillDeps<'_>) -> KillReport {
    match preflight(&input, deps) {
        Preflight::Aborted(abort) => abort_report(&input, abort),
        Preflight::TargetExited => target_exited_report(&input, deps),
        Preflight::Ready(plan) => run_kill(&input, plan, deps),
    }
}

struct Abort {
    code: ErrorCode,
    detail: Option<String>,
}

impl Abort {
    fn code(code: ErrorCode) -> Self {
        Self { code, detail: None }
    }

    /// Pre-flight refusals keep the canonical `aborted` code while the human
    /// message carries the specific safety reason.
    fn reason(reason: ErrorCode, port: u16, pid: u32) -> Self {
        Self {
            code: ErrorCode::AccessDenied,
            detail: Some(reason.message(port, Some(pid))),
        }
    }
}

enum Preflight {
    Aborted(Abort),
    TargetExited,
    Ready(Vec<(ProcessRef, TermToken)>),
}

fn preflight(input: &KillInput, deps: &mut KillDeps<'_>) -> Preflight {
    let mut plan: Vec<(ProcessRef, TermToken)> = Vec::new();

    for (index, member) in input.kill_set.iter().enumerate() {
        let is_target = index == 0;

        let Some(info) = input.snapshot.get(member.pid) else {
            if is_target {
                cleanup(&mut deps.system, &plan, input.target_query);
                return Preflight::TargetExited;
            }
            continue; // a descendant that no longer exists is dropped
        };

        if info.started_at_unix != Some(member.started_at_unix) {
            cleanup(&mut deps.system, &plan, input.target_query);
            return Preflight::Aborted(Abort::code(ErrorCode::StaleTarget));
        }

        let query = if is_target {
            input.target_query
        } else {
            match deps.system.open_query(member.pid) {
                Ok(token) => token,
                Err(InspectError::AccessDenied) => {
                    cleanup(&mut deps.system, &plan, input.target_query);
                    return Preflight::Aborted(Abort::code(ErrorCode::AccessDenied));
                }
                Err(InspectError::Unreadable) => {
                    // The PID is gone (or unreadable): drop the descendant.
                    continue;
                }
            }
        };

        let creation = deps.system.creation_time_unix(query);
        if is_target && !deps.system.is_alive(query) {
            cleanup(&mut deps.system, &plan, input.target_query);
            return Preflight::TargetExited;
        }

        if !is_target && !deps.system.is_alive(query) {
            // A descendant that exited between the snapshot and pre-flight is
            // dropped, never a reason to abort the whole kill.
            deps.system.close_query(query);
            continue;
        }

        match creation {
            Ok(started) if started == member.started_at_unix => {}
            Ok(_) => {
                if !is_target {
                    deps.system.close_query(query);
                }
                cleanup(&mut deps.system, &plan, input.target_query);
                return Preflight::Aborted(Abort::code(ErrorCode::StaleTarget));
            }
            Err(InspectError::AccessDenied) | Err(InspectError::Unreadable) => {
                if !is_target {
                    deps.system.close_query(query);
                }
                cleanup(&mut deps.system, &plan, input.target_query);
                return Preflight::Aborted(Abort::reason(
                    ErrorCode::NotInspectable,
                    input.port,
                    member.pid,
                ));
            }
        }

        if let Err(reason) = deps.system.safety(member.pid, query, &input.snapshot) {
            if !is_target {
                deps.system.close_query(query);
            }
            cleanup(&mut deps.system, &plan, input.target_query);
            return Preflight::Aborted(Abort::reason(reason, input.port, member.pid));
        }

        if is_target && !deps.system.is_alive(query) {
            cleanup(&mut deps.system, &plan, input.target_query);
            return Preflight::TargetExited;
        }

        match deps.system.open_terminate(member.pid) {
            Ok(token) => plan.push((*member, token)),
            Err(InspectError::AccessDenied) => {
                if !is_target {
                    deps.system.close_query(query);
                }
                cleanup(&mut deps.system, &plan, input.target_query);
                return Preflight::Aborted(Abort::code(ErrorCode::AccessDenied));
            }
            Err(InspectError::Unreadable) => {
                if !is_target {
                    deps.system.close_query(query);
                }
                cleanup(&mut deps.system, &plan, input.target_query);
                return Preflight::Aborted(Abort::reason(
                    ErrorCode::NotInspectable,
                    input.port,
                    member.pid,
                ));
            }
        }

        if !is_target {
            deps.system.close_query(query);
        }
    }

    // A target that exited between the last check and the end of pre-flight is
    // still `target_exited`, not a failed kill.
    if !deps.system.is_alive(input.target_query) {
        cleanup(&mut deps.system, &plan, input.target_query);
        return Preflight::TargetExited;
    }

    deps.system.close_query(input.target_query);
    Preflight::Ready(plan)
}

fn cleanup(
    system: &mut &mut dyn ProcessSystem,
    plan: &[(ProcessRef, TermToken)],
    target_query: QueryToken,
) {
    for (_, token) in plan {
        system.close_terminate(*token);
    }
    system.close_query(target_query);
}

fn run_kill(
    input: &KillInput,
    plan: Vec<(ProcessRef, TermToken)>,
    deps: &mut KillDeps<'_>,
) -> KillReport {
    let would_kill: Vec<ProcessRef> = plan.iter().map(|(member, _)| *member).collect();
    let mut killed_candidates = Vec::new();
    let mut termination_error = None;

    // The target is first in the plan and therefore runs first.
    for (member, token) in &plan {
        match deps.system.terminate(*token) {
            Ok(()) => killed_candidates.push(*member),
            Err(error) => {
                if termination_error.is_none() {
                    termination_error = Some(error);
                }
            }
        }
    }

    let survivors = wait_for_exit(&plan, deps.system, deps.clock);
    for (_, token) in &plan {
        deps.system.close_terminate(*token);
    }

    let killed: Vec<ProcessRef> = killed_candidates
        .into_iter()
        .filter(|member| !survivors.contains(member))
        .collect();

    let observed = observe(input.port, &input.preexisting, &input.kill_set, deps);

    let error_code = if !survivors.is_empty() {
        Some(ErrorCode::Survivor)
    } else if !observed.respawned.is_empty() {
        Some(ErrorCode::Respawned)
    } else if !observed.free || !observed.held.is_empty() {
        Some(ErrorCode::PortStillHeld)
    } else if termination_error.is_some() {
        Some(ErrorCode::KillFailed)
    } else {
        None
    };

    let outcome = if error_code.is_some() {
        Outcome::Failed
    } else {
        Outcome::Killed
    };

    KillReport {
        outcome,
        would_kill,
        killed,
        survivors,
        respawned: Some(observed.respawned),
        port_held_by: Some(observed.held),
        port_free: Some(observed.free),
        error: error_code.map(|code| ErrorInfo::for_code(code, input.port, Some(input.target.pid))),
    }
}

fn wait_for_exit(
    plan: &[(ProcessRef, TermToken)],
    system: &mut dyn ProcessSystem,
    clock: &dyn Clock,
) -> Vec<ProcessRef> {
    let deadline = clock.now_ms() + SURVIVOR_WAIT_MS;
    loop {
        let alive: Vec<ProcessRef> = plan
            .iter()
            .filter(|(_, token)| system.is_alive_term(*token))
            .map(|(member, _)| *member)
            .collect();
        if alive.is_empty() {
            return Vec::new();
        }
        let now = clock.now_ms();
        if now >= deadline {
            return alive;
        }
        let remaining = deadline - now;
        clock.sleep(Duration::from_millis(WAIT_POLL_MS.min(remaining.max(1))));
    }
}

fn target_exited_report(input: &KillInput, deps: &mut KillDeps<'_>) -> KillReport {
    let observed = observe(input.port, &input.preexisting, &input.kill_set, deps);

    let error_code = if !observed.respawned.is_empty() {
        Some(ErrorCode::Respawned)
    } else if !observed.free || !observed.held.is_empty() {
        Some(ErrorCode::PortStillHeld)
    } else {
        None
    };

    let outcome = if error_code.is_some() {
        Outcome::Failed
    } else {
        Outcome::TargetExited
    };

    KillReport {
        outcome,
        would_kill: input.kill_set.clone(),
        killed: Vec::new(),
        survivors: Vec::new(),
        respawned: Some(observed.respawned),
        port_held_by: Some(observed.held),
        port_free: Some(observed.free),
        error: error_code.map(|code| ErrorInfo::for_code(code, input.port, Some(input.target.pid))),
    }
}

fn abort_report(input: &KillInput, abort: Abort) -> KillReport {
    let message = abort
        .detail
        .unwrap_or_else(|| abort.code.message(input.port, Some(input.target.pid)));
    KillReport {
        outcome: Outcome::Aborted,
        would_kill: input.kill_set.clone(),
        killed: Vec::new(),
        survivors: Vec::new(),
        respawned: None,
        port_held_by: None,
        port_free: None,
        error: Some(ErrorInfo {
            code: abort.code,
            message,
        }),
    }
}

struct Observed {
    respawned: Vec<Respawned>,
    held: Vec<ProcessRef>,
    free: bool,
}

fn observe(
    port: u16,
    preexisting: &[Owner],
    kill_set: &[ProcessRef],
    deps: &mut KillDeps<'_>,
) -> Observed {
    let start = deps.clock.now_ms();
    let mut new_owners: BTreeMap<(u32, Option<i64>), Respawned> = BTreeMap::new();

    loop {
        let now = deps.clock.now_ms();
        let last = now.saturating_sub(start) >= OBSERVE_MS;

        let mut table_failed = false;
        let rows = match deps.ports.tcp_rows() {
            Ok(rows) => rows,
            Err(_) => {
                table_failed = true;
                Vec::new()
            }
        };
        let listeners: Vec<&crate::model::TcpRow> = rows
            .iter()
            .filter(|row| row.state == RowState::Listen && row.port == port)
            .collect();

        let mut pids: BTreeSet<u32> = BTreeSet::new();
        for row in &listeners {
            for pid in row.pids.iter().copied().filter(|pid| *pid != 0) {
                pids.insert(pid);
            }
        }

        // One process snapshot per poll gives every owner an exact start time,
        // so a recycled PID is a new identity, not a survivor or a co-listener.
        let snapshot = if pids.is_empty() {
            None
        } else {
            deps.processes.snapshot().ok()
        };
        let mut owners: BTreeSet<(u32, Option<i64>)> = BTreeSet::new();
        for pid in pids {
            let started = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.get(pid))
                .and_then(|info| info.started_at_unix)
                .or_else(|| {
                    preexisting
                        .iter()
                        .find(|owner| owner.pid == pid)
                        .and_then(|owner| owner.started_at_unix)
                });
            owners.insert((pid, started));
        }

        for (pid, started) in &owners {
            if is_preexisting(*pid, *started, preexisting)
                || is_kill_member(*pid, *started, kill_set)
            {
                continue;
            }
            new_owners
                .entry((*pid, *started))
                .or_insert_with(|| respawned_entry(snapshot.as_ref(), *pid, *started));
        }

        if last {
            let mut held = Vec::new();
            for (pid, started) in &owners {
                if is_preexisting(*pid, *started, preexisting)
                    && !is_kill_member(*pid, *started, kill_set)
                    && let Some(started) = started
                {
                    held.push(ProcessRef {
                        pid: *pid,
                        started_at_unix: *started,
                    });
                }
            }
            held.sort_unstable();
            held.dedup();
            return Observed {
                respawned: new_owners.into_values().collect(),
                held,
                free: listeners.is_empty() && !table_failed,
            };
        }

        let elapsed = now.saturating_sub(start);
        let remaining = OBSERVE_MS.saturating_sub(elapsed).max(1);
        deps.clock
            .sleep(Duration::from_millis(OBSERVE_POLL_MS.min(remaining)));
    }
}

fn is_preexisting(pid: u32, started: Option<i64>, preexisting: &[Owner]) -> bool {
    preexisting.iter().any(|o| {
        o.pid == pid
            && match (o.started_at_unix, started) {
                (Some(a), Some(b)) => a == b,
                _ => true,
            }
    })
}

fn is_kill_member(pid: u32, started: Option<i64>, kill_set: &[ProcessRef]) -> bool {
    kill_set.iter().any(|k| {
        k.pid == pid
            && match started {
                Some(started) => k.started_at_unix == started,
                None => true,
            }
    })
}

fn respawned_entry(
    snapshot: Option<&ProcessSnapshot>,
    pid: u32,
    started: Option<i64>,
) -> Respawned {
    let info = snapshot.and_then(|snapshot| snapshot.get(pid));
    Respawned {
        pid,
        started_at_unix: info.and_then(|p| p.started_at_unix).or(started),
        name: info.and_then(|p| p.name.clone()),
        exe: info.and_then(|p| p.exe.clone()),
        cmdline: info.and_then(|p| p.cmdline.clone()),
    }
}

/// Convenience for the app: a dry-run report with a predicted kill set.
pub fn dry_run_report(
    would_kill: Vec<ProcessRef>,
    preexisting: &[Owner],
    kill_set: &[ProcessRef],
) -> KillReport {
    let held: Vec<ProcessRef> = preexisting
        .iter()
        .filter(|owner| !kill_set.iter().any(|member| member.pid == owner.pid))
        .filter_map(|owner| {
            owner.started_at_unix.map(|started_at_unix| ProcessRef {
                pid: owner.pid,
                started_at_unix,
            })
        })
        .collect();
    KillReport {
        outcome: Outcome::DryRun,
        would_kill,
        killed: Vec::new(),
        survivors: Vec::new(),
        respawned: None,
        port_held_by: Some(held),
        port_free: None,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::collections::VecDeque;
    use std::net::{IpAddr, Ipv4Addr};

    use crate::model::{Family, ProcessInfo, RowState, TcpRow};
    use crate::ports::TableError;
    use crate::system::TerminateError;

    struct FakeClock {
        ms: Cell<u64>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self { ms: Cell::new(0) }
        }
    }

    impl Clock for FakeClock {
        fn now_unix(&self) -> i64 {
            1_000
        }

        fn now_ms(&self) -> u64 {
            self.ms.get()
        }

        fn sleep(&self, duration: Duration) {
            self.ms.set(self.ms.get() + duration.as_millis() as u64);
        }
    }

    struct FakePortTable {
        rows: RefCell<Vec<Vec<TcpRow>>>,
        fallback: Vec<TcpRow>,
    }

    impl FakePortTable {
        fn new(rows: Vec<Vec<TcpRow>>, fallback: Vec<TcpRow>) -> Self {
            Self {
                rows: RefCell::new(rows),
                fallback,
            }
        }
    }

    impl PortTable for FakePortTable {
        fn tcp_rows(&mut self) -> Result<Vec<TcpRow>, TableError> {
            let mut rows = self.rows.borrow_mut();
            if rows.is_empty() {
                Ok(self.fallback.clone())
            } else {
                Ok(rows.remove(0))
            }
        }
    }

    struct FakeProcessTable {
        snapshots: VecDeque<ProcessSnapshot>,
        fallback: ProcessSnapshot,
    }

    impl ProcessTable for FakeProcessTable {
        fn snapshot(&mut self) -> Result<ProcessSnapshot, TableError> {
            if self.snapshots.len() > 1 {
                Ok(self.snapshots.pop_front().unwrap())
            } else {
                Ok(self
                    .snapshots
                    .front()
                    .cloned()
                    .unwrap_or_else(|| self.fallback.clone()))
            }
        }
    }

    #[derive(Default)]
    struct FakeSystem {
        next: u64,
        queries: HashMap<u64, (u32, i64)>,
        terms: HashMap<u64, u32>,
        starts: HashMap<u32, i64>,
        alive: HashMap<u32, bool>,
        safety: HashMap<u32, ErrorCode>,
        fail_query: HashMap<u32, InspectError>,
        fail_terminate: HashMap<u32, InspectError>,
        terminate_error: HashMap<u32, TerminateError>,
        terminated: RefCell<Vec<u32>>,
    }

    impl FakeSystem {
        fn with_targets(targets: &[(u32, i64)]) -> Self {
            let mut system = Self::default();
            for (pid, start) in targets {
                system.starts.insert(*pid, *start);
                system.alive.insert(*pid, true);
            }
            system
        }
    }

    impl ProcessSystem for FakeSystem {
        fn open_query(&mut self, pid: u32) -> Result<QueryToken, InspectError> {
            if let Some(error) = self.fail_query.get(&pid) {
                return Err(*error);
            }
            let Some(start) = self.starts.get(&pid).copied() else {
                return Err(InspectError::Unreadable);
            };
            let token = self.next;
            self.next += 1;
            self.queries.insert(token, (pid, start));
            Ok(QueryToken(token))
        }

        fn close_query(&mut self, token: QueryToken) {
            self.queries.remove(&token.0);
        }

        fn creation_time_unix(&self, token: QueryToken) -> Result<i64, InspectError> {
            self.queries
                .get(&token.0)
                .map(|(_, start)| *start)
                .ok_or(InspectError::Unreadable)
        }

        fn is_alive(&self, token: QueryToken) -> bool {
            self.queries
                .get(&token.0)
                .and_then(|(pid, _)| self.alive.get(pid))
                .copied()
                .unwrap_or(false)
        }

        fn safety(
            &self,
            pid: u32,
            _token: QueryToken,
            _snapshot: &ProcessSnapshot,
        ) -> Result<(), ErrorCode> {
            match self.safety.get(&pid) {
                Some(code) => Err(*code),
                None => Ok(()),
            }
        }

        fn open_terminate(&mut self, pid: u32) -> Result<TermToken, InspectError> {
            if let Some(error) = self.fail_terminate.get(&pid) {
                return Err(*error);
            }
            let token = self.next;
            self.next += 1;
            self.terms.insert(token, pid);
            Ok(TermToken(token))
        }

        fn close_terminate(&mut self, token: TermToken) {
            self.terms.remove(&token.0);
        }

        fn terminate(&mut self, token: TermToken) -> Result<(), TerminateError> {
            let Some(pid) = self.terms.get(&token.0).copied() else {
                return Err(TerminateError::Failed);
            };
            if let Some(error) = self.terminate_error.get(&pid) {
                return Err(*error);
            }
            self.terminated.borrow_mut().push(pid);
            self.alive.insert(pid, false);
            Ok(())
        }

        fn is_alive_term(&self, token: TermToken) -> bool {
            self.terms
                .get(&token.0)
                .and_then(|pid| self.alive.get(pid))
                .copied()
                .unwrap_or(false)
        }
    }

    fn process(pid: u32, parent: Option<u32>, start: i64) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent,
            name: Some(format!("p{pid}.exe")),
            exe: Some(format!("C:\\work\\p{pid}.exe")),
            cmdline: Some(vec![format!("p{pid}.exe")]),
            cwd: Some("C:\\work".into()),
            started_at_unix: Some(start),
        }
    }

    fn snapshot(entries: &[ProcessInfo]) -> ProcessSnapshot {
        crate::resolve::snapshot_from(entries.iter().cloned(), 1_000)
    }

    fn listener(port: u16, pids: &[u32]) -> TcpRow {
        TcpRow {
            family: Family::Ipv4,
            address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port,
            state: RowState::Listen,
            pids: pids.to_vec(),
        }
    }

    fn base_input() -> (KillInput, FakeSystem) {
        let target = ProcessRef {
            pid: 10,
            started_at_unix: 100,
        };
        let child = ProcessRef {
            pid: 11,
            started_at_unix: 101,
        };
        let mut system = FakeSystem::with_targets(&[(10, 100), (11, 101)]);
        let query = system.open_query(10).unwrap();
        (
            KillInput {
                port: 3000,
                target,
                kill_set: vec![target, child],
                preexisting: vec![Owner {
                    pid: 10,
                    started_at_unix: Some(100),
                }],
                target_query: query,
                snapshot: snapshot(&[process(10, Some(1), 100), process(11, Some(10), 101)]),
            },
            system,
        )
    }

    fn run(
        input: KillInput,
        system: &mut FakeSystem,
        ports: &mut FakePortTable,
        processes: &mut FakeProcessTable,
        clock: &FakeClock,
    ) -> KillReport {
        let mut deps = KillDeps {
            ports,
            processes,
            system,
            clock,
        };
        execute(input, &mut deps)
    }

    fn empty_process_table() -> FakeProcessTable {
        FakeProcessTable {
            snapshots: VecDeque::new(),
            fallback: snapshot(&[]),
        }
    }

    #[test]
    fn kills_the_target_first_then_descendants_and_reports_free() {
        let (input, mut system) = base_input();
        let mut ports = FakePortTable::new(Vec::new(), Vec::new());
        let mut processes = empty_process_table();
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::Killed);
        assert_eq!(
            report.killed,
            vec![
                ProcessRef {
                    pid: 10,
                    started_at_unix: 100
                },
                ProcessRef {
                    pid: 11,
                    started_at_unix: 101
                },
            ]
        );
        assert_eq!(system.terminated.borrow().as_slice(), &[10, 11]);
        assert_eq!(report.survivors, Vec::<ProcessRef>::new());
        assert_eq!(report.respawned, Some(Vec::new()));
        assert_eq!(report.port_held_by, Some(Vec::new()));
        assert_eq!(report.port_free, Some(true));
        assert!(report.error.is_none());
    }

    #[test]
    fn a_survivor_fails_the_kill_and_preempts_respawn() {
        let (input, mut system) = base_input();
        system.terminate_error.insert(11, TerminateError::Failed);
        let mut ports = FakePortTable::new(Vec::new(), vec![listener(3000, &[99])]);
        let mut processes = FakeProcessTable {
            snapshots: VecDeque::from([snapshot(&[process(99, Some(1), 900)])]),
            fallback: snapshot(&[]),
        };
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Survivor);
        assert_eq!(
            report.survivors,
            vec![ProcessRef {
                pid: 11,
                started_at_unix: 101
            }]
        );
        assert_eq!(
            report.killed,
            vec![ProcessRef {
                pid: 10,
                started_at_unix: 100
            }]
        );
    }

    #[test]
    fn a_new_listener_is_a_respawn() {
        let (input, mut system) = base_input();
        let mut processes = FakeProcessTable {
            snapshots: VecDeque::from([snapshot(&[process(99, Some(1), 900)])]),
            fallback: snapshot(&[]),
        };
        // Free for the first few polls, then a new owner appears.
        let mut rows = vec![Vec::new(), Vec::new(), vec![listener(3000, &[99])]];
        let mut ports = FakePortTable::new(std::mem::take(&mut rows), vec![listener(3000, &[99])]);
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Respawned);
        assert_eq!(report.respawned.as_ref().unwrap().len(), 1);
        assert_eq!(report.respawned.as_ref().unwrap()[0].pid, 99);
        assert_eq!(
            report.respawned.as_ref().unwrap()[0].started_at_unix,
            Some(900)
        );
        assert_eq!(report.port_free, Some(false));
    }

    #[test]
    fn a_preexisting_co_listener_is_port_still_held() {
        let (input, mut system) = base_input();
        let mut input = input;
        input.preexisting.push(Owner {
            pid: 20,
            started_at_unix: Some(200),
        });
        let mut ports = FakePortTable::new(Vec::new(), vec![listener(3000, &[20])]);
        let mut processes = empty_process_table();
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(
            report.error.as_ref().unwrap().code,
            ErrorCode::PortStillHeld
        );
        assert_eq!(
            report.port_held_by,
            Some(vec![ProcessRef {
                pid: 20,
                started_at_unix: 200
            }])
        );
        assert_eq!(report.respawned, Some(Vec::new()));
        assert_eq!(report.port_free, Some(false));
    }

    #[test]
    fn a_target_that_already_exited_skips_the_kill_and_observes_the_port() {
        let (input, mut system) = base_input();
        system.alive.insert(10, false);
        let mut ports = FakePortTable::new(Vec::new(), Vec::new());
        let mut processes = empty_process_table();
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::TargetExited);
        assert!(system.terminated.borrow().is_empty());
        assert_eq!(report.port_free, Some(true));
        assert_eq!(report.port_held_by, Some(Vec::new()));
    }

    #[test]
    fn a_missing_descendant_is_dropped() {
        let (mut input, mut system) = base_input();
        input.snapshot.processes.remove(&11);
        let mut ports = FakePortTable::new(Vec::new(), Vec::new());
        let mut processes = empty_process_table();
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::Killed);
        assert_eq!(system.terminated.borrow().as_slice(), &[10]);
        assert_eq!(
            report.would_kill,
            vec![ProcessRef {
                pid: 10,
                started_at_unix: 100
            }]
        );
    }

    #[test]
    fn a_recycled_pid_is_a_respawn_not_a_survivor() {
        let (input, mut system) = base_input();
        let mut ports = FakePortTable::new(Vec::new(), vec![listener(3000, &[10])]);
        let mut processes = FakeProcessTable {
            snapshots: VecDeque::from([snapshot(&[process(10, Some(1), 999)])]),
            fallback: snapshot(&[]),
        };
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::Respawned);
        assert_eq!(
            report.respawned.as_ref().unwrap()[0].started_at_unix,
            Some(999)
        );
        assert!(report.survivors.is_empty());
    }

    #[test]
    fn an_access_failure_in_preflight_aborts_without_killing() {
        let (input, mut system) = base_input();
        system.fail_terminate.insert(11, InspectError::AccessDenied);
        let mut ports = FakePortTable::new(Vec::new(), Vec::new());
        let mut processes = empty_process_table();
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::Aborted);
        assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::AccessDenied);
        assert!(system.terminated.borrow().is_empty());
        assert_eq!(report.port_free, None);
        assert_eq!(report.port_held_by, None);
        assert_eq!(report.respawned, None);
    }

    #[test]
    fn a_descendant_safety_refusal_aborts_without_killing() {
        let (input, mut system) = base_input();
        system.safety.insert(11, ErrorCode::CriticalProcess);
        let mut ports = FakePortTable::new(Vec::new(), Vec::new());
        let mut processes = empty_process_table();
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::Aborted);
        let error = report.error.as_ref().unwrap();
        assert_eq!(error.code, ErrorCode::AccessDenied);
        assert!(
            error.message.contains("critical"),
            "message should carry the safety reason: {}",
            error.message
        );
        assert!(system.terminated.borrow().is_empty());
    }

    #[test]
    fn a_preflight_creation_time_mismatch_is_stale() {
        let (input, mut system) = base_input();
        system.queries.insert(input.target_query.0, (10, 999));
        let mut ports = FakePortTable::new(Vec::new(), Vec::new());
        let mut processes = empty_process_table();
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::Aborted);
        assert_eq!(report.error.as_ref().unwrap().code, ErrorCode::StaleTarget);
    }

    #[test]
    fn a_pid_zero_listener_during_observation_is_not_free() {
        let (input, mut system) = base_input();
        let mut ports = FakePortTable::new(Vec::new(), vec![listener(3000, &[0])]);
        let mut processes = empty_process_table();
        let clock = FakeClock::new();

        let report = run(input, &mut system, &mut ports, &mut processes, &clock);

        assert_eq!(report.outcome, Outcome::Failed);
        assert_eq!(
            report.error.as_ref().unwrap().code,
            ErrorCode::PortStillHeld
        );
        assert_eq!(report.port_free, Some(false));
        assert_eq!(report.port_held_by, Some(Vec::new()));
    }
}
