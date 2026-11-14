//! Windows process handles and safety checks.
//!
//! The target query handle is opened during inspection and held across the
//! prompt. A terminate handle is opened only after consent.

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, FILETIME, GetLastError, HANDLE, STILL_ACTIVE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::{
    CreateWellKnownSid, EqualSid, GetLengthSid, GetTokenInformation, IsValidSid, PSID,
    SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER, TokenUser, WinLocalServiceSid,
    WinLocalSystemSid, WinNetworkServiceSid,
};
use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, IsProcessCritical, OpenProcess,
    OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    TerminateProcess, WaitForSingleObject,
};

use crate::model::{ErrorCode, ProcessSnapshot};
use crate::processes::filetime_to_unix_seconds;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryToken(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermToken(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InspectError {
    AccessDenied,
    Unreadable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminateError {
    AccessDenied,
    Failed,
}

/// Handle operations and safety checks. Fakeable so kill-batch logic can be
/// tested without spawning system processes.
pub trait ProcessSystem {
    fn open_query(&mut self, pid: u32) -> Result<QueryToken, InspectError>;
    fn close_query(&mut self, token: QueryToken);
    fn creation_time_unix(&self, token: QueryToken) -> Result<i64, InspectError>;
    fn is_alive(&self, token: QueryToken) -> bool;
    fn safety(
        &self,
        pid: u32,
        token: QueryToken,
        snapshot: &ProcessSnapshot,
    ) -> Result<(), ErrorCode>;

    fn open_terminate(&mut self, pid: u32) -> Result<TermToken, InspectError>;
    fn close_terminate(&mut self, token: TermToken);
    fn terminate(&mut self, token: TermToken) -> Result<(), TerminateError>;
    fn is_alive_term(&self, token: TermToken) -> bool;
}

pub struct WindowsSystem {
    next_token: u64,
    queries: HashMap<u64, OwnedHandle>,
    terms: HashMap<u64, OwnedHandle>,
    current_pid: u32,
    current_sid: OnceLock<Option<Vec<u64>>>,
    windows_dir: OnceLock<Option<String>>,
}

impl WindowsSystem {
    pub fn new() -> Self {
        Self {
            next_token: 1,
            queries: HashMap::new(),
            terms: HashMap::new(),
            current_pid: std::process::id(),
            current_sid: OnceLock::new(),
            windows_dir: OnceLock::new(),
        }
    }

    fn allocate(&mut self) -> u64 {
        let token = self.next_token;
        self.next_token += 1;
        token
    }

    fn current_sid(&self) -> Option<&[u64]> {
        self.current_sid
            .get_or_init(|| token_user_sid_copy(unsafe { GetCurrentProcess() }))
            .as_deref()
    }

    fn canonical_windows_dir(&self) -> Option<&str> {
        self.windows_dir
            .get_or_init(|| {
                system_windows_directory().map(|dir| {
                    let long = long_path(&dir).unwrap_or(dir);
                    normalize_path(&long)
                })
            })
            .as_deref()
    }
}

impl Default for WindowsSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessSystem for WindowsSystem {
    fn open_query(&mut self, pid: u32) -> Result<QueryToken, InspectError> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return Err(inspect_error_from_last_error());
        }
        let token = self.allocate();
        self.queries.insert(token, unsafe {
            OwnedHandle::from_raw_handle(handle as RawHandle)
        });
        Ok(QueryToken(token))
    }

    fn close_query(&mut self, token: QueryToken) {
        self.queries.remove(&token.0);
    }

    fn creation_time_unix(&self, token: QueryToken) -> Result<i64, InspectError> {
        let handle = self
            .queries
            .get(&token.0)
            .ok_or(InspectError::Unreadable)?
            .as_raw_handle() as HANDLE;
        process_creation_time(handle).ok_or(InspectError::Unreadable)
    }

    fn is_alive(&self, token: QueryToken) -> bool {
        self.queries
            .get(&token.0)
            .map(|handle| process_is_alive(handle.as_raw_handle() as HANDLE))
            .unwrap_or(false)
    }

    fn safety(
        &self,
        pid: u32,
        token: QueryToken,
        snapshot: &ProcessSnapshot,
    ) -> Result<(), ErrorCode> {
        let handle = self
            .queries
            .get(&token.0)
            .ok_or(ErrorCode::NotInspectable)?
            .as_raw_handle() as HANDLE;

        // 1. Critical process. Checked before session 0 so a critical
        //    process is reported as critical_process. Hosts that do not
        //    report PID 4 as critical still refuse it via session 0.
        let mut critical = 0;
        if unsafe { IsProcessCritical(handle, &mut critical) } == 0 {
            return Err(ErrorCode::NotInspectable);
        }
        if critical != 0 {
            return Err(ErrorCode::CriticalProcess);
        }

        // 2. Session 0.
        let mut session = 0;
        if unsafe { ProcessIdToSessionId(pid, &mut session) } == 0 {
            return Err(ErrorCode::NotInspectable);
        }
        if session == 0 {
            return Err(ErrorCode::SessionZero);
        }

        // 3. Token SID: service account or another user.
        let target_sid = token_user_sid_copy(handle).ok_or(ErrorCode::NotInspectable)?;
        let current_sid = self.current_sid().ok_or(ErrorCode::NotInspectable)?;
        classify_sid(&target_sid, current_sid)?;

        // 4. Executable under the canonical Windows directory.
        let info = snapshot.get(pid).ok_or(ErrorCode::NotInspectable)?;
        let exe = info
            .exe
            .as_deref()
            .filter(|e| !e.is_empty())
            .ok_or(ErrorCode::NotInspectable)?;
        let windows_dir = self
            .canonical_windows_dir()
            .ok_or(ErrorCode::NotInspectable)?;
        let long_exe = long_path(exe).unwrap_or_else(|| exe.to_string());
        if is_under_directory(&long_exe, windows_dir) {
            return Err(ErrorCode::WindowsDirectory);
        }

        // 5. Current process or an ancestor of it.
        if pid == self.current_pid || snapshot.ancestors_of(self.current_pid).contains(&pid) {
            return Err(ErrorCode::AncestorProcess);
        }

        Ok(())
    }

    fn open_terminate(&mut self, pid: u32) -> Result<TermToken, InspectError> {
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return Err(inspect_error_from_last_error());
        }
        let token = self.allocate();
        self.terms.insert(token, unsafe {
            OwnedHandle::from_raw_handle(handle as RawHandle)
        });
        Ok(TermToken(token))
    }

    fn close_terminate(&mut self, token: TermToken) {
        self.terms.remove(&token.0);
    }

    fn terminate(&mut self, token: TermToken) -> Result<(), TerminateError> {
        let Some(handle) = self.terms.get(&token.0) else {
            return Err(TerminateError::Failed);
        };
        if unsafe { TerminateProcess(handle.as_raw_handle() as HANDLE, 1) } == 0 {
            let error = unsafe { GetLastError() };
            return Err(if error == ERROR_ACCESS_DENIED {
                TerminateError::AccessDenied
            } else {
                TerminateError::Failed
            });
        }
        Ok(())
    }

    fn is_alive_term(&self, token: TermToken) -> bool {
        self.terms
            .get(&token.0)
            .map(|handle| wait_is_alive(handle.as_raw_handle() as HANDLE))
            .unwrap_or(false)
    }
}

fn inspect_error_from_last_error() -> InspectError {
    if unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
        InspectError::AccessDenied
    } else {
        InspectError::Unreadable
    }
}

fn process_is_alive(handle: HANDLE) -> bool {
    // A failed query is treated as alive so a kill is never reported optimistically.
    let mut code = 0u32;
    if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
        return true;
    }
    code == STILL_ACTIVE as u32
}

fn wait_is_alive(handle: HANDLE) -> bool {
    // A failed wait is treated as alive so a kill is never reported optimistically.
    match unsafe { WaitForSingleObject(handle, 0) } {
        WAIT_OBJECT_0 => false,
        WAIT_TIMEOUT => true,
        _ => true,
    }
}

fn process_creation_time(handle: HANDLE) -> Option<i64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let filetime = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
    if filetime == 0 {
        return None;
    }
    Some(filetime_to_unix_seconds(filetime))
}

fn token_user_sid_copy(process: HANDLE) -> Option<Vec<u64>> {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return None;
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token as RawHandle) };

    let mut length = 0u32;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle() as HANDLE,
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut length,
        )
    };
    if length == 0 {
        return None;
    }

    let words = (length as usize).div_ceil(8);
    let mut buffer = vec![0u64; words];
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle() as HANDLE,
            TokenUser,
            buffer.as_mut_ptr() as *mut c_void,
            length,
            &mut length,
        )
    } == 0
    {
        return None;
    }

    let token_user = buffer.as_ptr() as *const TOKEN_USER;
    let sid = unsafe { (*token_user).User.Sid };
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return None;
    }
    let sid_length = unsafe { GetLengthSid(sid) };
    if sid_length == 0 || sid_length as usize > SECURITY_MAX_SID_SIZE as usize {
        return None;
    }
    let sid_words = (sid_length as usize).div_ceil(8);
    let mut copy = vec![0u64; sid_words];
    if unsafe { windows_sys::Win32::Security::CopySid(sid_length, copy.as_mut_ptr() as PSID, sid) }
        == 0
    {
        return None;
    }
    Some(copy)
}

fn classify_sid(target: &[u64], current: &[u64]) -> Result<(), ErrorCode> {
    let target_ptr = target.as_ptr() as PSID;
    for well_known in [WinLocalSystemSid, WinLocalServiceSid, WinNetworkServiceSid] {
        let mut buffer = [0u64; (SECURITY_MAX_SID_SIZE as usize).div_ceil(8)];
        let mut size = SECURITY_MAX_SID_SIZE;
        let ok = unsafe {
            CreateWellKnownSid(
                well_known,
                std::ptr::null_mut(),
                buffer.as_mut_ptr() as PSID,
                &mut size,
            )
        };
        if ok == 0 {
            return Err(ErrorCode::NotInspectable);
        }
        if unsafe { EqualSid(target_ptr, buffer.as_mut_ptr() as PSID) } != 0 {
            return Err(ErrorCode::ServiceAccount);
        }
    }

    if unsafe { EqualSid(target_ptr, current.as_ptr() as PSID) } == 0 {
        return Err(ErrorCode::OtherUser);
    }
    Ok(())
}

fn system_windows_directory() -> Option<String> {
    let mut buffer: Vec<u16> = vec![0; 260];
    loop {
        let len = unsafe { GetSystemWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
        if len == 0 {
            return None;
        }
        if (len as usize) < buffer.len() {
            buffer.truncate(len as usize);
            return String::from_utf16(&buffer).ok();
        }
        buffer.resize(len as usize + 1, 0);
    }
}

fn long_path(path: &str) -> Option<String> {
    let input = wide(path);
    let mut buffer: Vec<u16> = vec![0; 260];
    loop {
        let len =
            unsafe { GetLongPathNameW(input.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
        if len == 0 {
            return None;
        }
        if (len as usize) < buffer.len() {
            buffer.truncate(len as usize);
            return String::from_utf16(&buffer).ok();
        }
        buffer.resize(len as usize + 1, 0);
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Remove a `\\?\` prefix (including the UNC form) and normalize separators.
pub fn normalize_path(path: &str) -> String {
    let mut value = path.trim().to_string();
    if let Some(rest) = value.strip_prefix("\\\\?\\UNC\\") {
        value = format!(r"\\{rest}");
    } else if let Some(rest) = value.strip_prefix("\\\\?\\") {
        value = rest.to_string();
    }
    while value.len() > 1 && (value.ends_with('\\') || value.ends_with('/')) {
        value.pop();
    }
    value.replace('/', "\\")
}

/// Component-wise, case-folded prefix comparison. `C:\Windows10Upgrade` is not
/// under `C:\Windows`; the comparison is strict (the directory itself is not an
/// executable path under it).
pub fn is_under_directory(path: &str, directory: &str) -> bool {
    let path = normalize_path(path);
    let directory = normalize_path(directory);
    let path_parts: Vec<String> = path
        .split('\\')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(|part| part.to_lowercase())
        .collect();
    let dir_parts: Vec<String> = directory
        .split('\\')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(|part| part.to_lowercase())
        .collect();
    path_parts.len() > dir_parts.len()
        && dir_parts.iter().zip(path_parts.iter()).all(|(a, b)| a == b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProcessInfo, ProcessSnapshot};
    use crate::processes::{ProcessTable, SysinfoProcessTable};
    use std::collections::BTreeMap;
    use windows_sys::Win32::Security::WinWorldSid;

    #[test]
    fn windows_directory_comparison_is_component_wise() {
        assert!(is_under_directory(
            r"C:\Windows\System32\notepad.exe",
            r"C:\Windows"
        ));
        assert!(is_under_directory(
            r"c:\windows\system32\NOTEPAD.EXE",
            r"C:\Windows"
        ));
        assert!(!is_under_directory(
            r"C:\Windows10Upgrade\setup.exe",
            r"C:\Windows"
        ));
        assert!(!is_under_directory(r"C:\Windows", r"C:\Windows"));
        assert!(!is_under_directory(r"C:\WindowsOld\x.exe", r"C:\Windows"));
    }

    #[test]
    fn verbatim_and_unc_prefixes_are_normalized() {
        assert_eq!(
            normalize_path(r"\\?\C:\Windows\System32"),
            r"C:\Windows\System32"
        );
        assert_eq!(
            normalize_path(r"\\?\UNC\server\share\x.exe"),
            r"\\server\share\x.exe"
        );
        assert!(is_under_directory(
            r"\\?\C:\Windows\System32\x.exe",
            r"C:\Windows"
        ));
    }

    #[test]
    fn service_sids_and_other_users_are_classified() {
        let local_system = sid_for(WinLocalSystemSid);
        let local_service = sid_for(WinLocalServiceSid);
        let network_service = sid_for(WinNetworkServiceSid);
        let current = sid_for(WinWorldSid);

        assert_eq!(
            classify_sid(&local_system, &current),
            Err(ErrorCode::ServiceAccount)
        );
        assert_eq!(
            classify_sid(&local_service, &current),
            Err(ErrorCode::ServiceAccount)
        );
        assert_eq!(
            classify_sid(&network_service, &current),
            Err(ErrorCode::ServiceAccount)
        );
        assert_eq!(classify_sid(&current, &current), Ok(()));
        assert_eq!(
            classify_sid(&sid_for(WinWorldSid), &local_system),
            Err(ErrorCode::OtherUser)
        );
    }

    fn sid_for(kind: i32) -> Vec<u64> {
        let mut buffer = [0u64; (SECURITY_MAX_SID_SIZE as usize).div_ceil(8)];
        let mut size = SECURITY_MAX_SID_SIZE;
        let ok = unsafe {
            CreateWellKnownSid(
                kind,
                std::ptr::null_mut(),
                buffer.as_mut_ptr() as PSID,
                &mut size,
            )
        };
        assert_ne!(ok, 0);
        buffer.to_vec()
    }

    fn snapshot_for(pid: u32, exe: &str) -> ProcessSnapshot {
        let mut processes = BTreeMap::new();
        processes.insert(
            pid,
            ProcessInfo {
                pid,
                exe: Some(exe.into()),
                ..ProcessInfo::default()
            },
        );
        ProcessSnapshot {
            processes,
            now_unix: 0,
        }
    }

    #[test]
    fn pid_four_is_refused() {
        let mut system = WindowsSystem::new();
        let pid = 4;
        match system.open_query(pid) {
            Ok(token) => {
                let snapshot = snapshot_for(pid, r"C:\Windows\System32\ntoskrnl.exe");
                // Some hosts (GitHub-hosted runners among them) report PID 4
                // as non-critical, so the session 0 check refuses it instead.
                let handle = system
                    .queries
                    .get(&token.0)
                    .expect("query handle")
                    .as_raw_handle() as HANDLE;
                let mut critical = 0;
                let ok = unsafe { IsProcessCritical(handle, &mut critical) };
                assert_ne!(ok, 0);
                let expected = if critical != 0 {
                    ErrorCode::CriticalProcess
                } else {
                    ErrorCode::SessionZero
                };
                assert_eq!(system.safety(pid, token, &snapshot), Err(expected));
            }
            // PID 4 is protected by policy in some environments. Refusing to
            // inspect it is the same safety outcome: nothing is killed.
            Err(error) => assert_eq!(error, InspectError::AccessDenied),
        }
    }

    #[test]
    fn the_current_process_is_an_ancestor_refusal() {
        let mut system = WindowsSystem::new();
        let pid = std::process::id();
        let token = system.open_query(pid).expect("open self");
        let snapshot = snapshot_for(pid, r"C:\work\portfree.exe");
        assert_eq!(
            system.safety(pid, token, &snapshot),
            Err(ErrorCode::AncestorProcess)
        );
    }

    #[test]
    fn sysinfo_start_time_and_get_process_times_agree_under_truncation() {
        let mut table = SysinfoProcessTable::new().expect("sysinfo");
        let snapshot = table.snapshot().expect("snapshot");
        let pid = std::process::id();
        let info = snapshot.get(pid).expect("self in snapshot");
        let expected = info.started_at_unix.expect("syinfo start time");

        let mut system = WindowsSystem::new();
        let token = system.open_query(pid).expect("open self");
        let actual = system.creation_time_unix(token).expect("creation time");
        assert_eq!(actual, expected);
    }
}
