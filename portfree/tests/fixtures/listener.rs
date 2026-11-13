//! Test fixture: a listener that portfree can find and kill.
//!
//! Modes:
//!   listen  --port N [--reuse] [--dual] [--extra N]
//!   respawn --port N --after-pid PID
//!
//! It prints `PORT=<port>` (and `EXTRA_PORT=<port>` per extra listener) then
//! holds the sockets open. `respawn` waits for the given PID to exit, then binds
//! with SO_REUSEADDR so the observation window can catch it.

use std::io::Write;
use std::net::TcpListener;
use std::os::windows::io::FromRawSocket;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
use windows_sys::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, IN_ADDR, IN6_ADDR, IPPROTO_IPV6, IPPROTO_TCP, IPV6_V6ONLY, SO_REUSEADDR,
    SOCK_STREAM, SOCKADDR_IN, SOCKADDR_IN6, SOL_SOCKET, WSADATA, WSAStartup, bind, listen,
    setsockopt, socket,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

fn main() {
    let mut mode = String::from("listen");
    let mut port = 0u16;
    let mut reuse = false;
    let mut dual = false;
    let mut extra = 0u32;
    let mut after_pid = 0u32;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--mode" => mode = args.next().unwrap_or_default(),
            "--port" => port = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--reuse" => reuse = true,
            "--dual" => dual = true,
            "--extra" => extra = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--after-pid" => after_pid = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            _ => {}
        }
    }

    unsafe {
        let mut data = WSADATA::default();
        assert_eq!(WSAStartup(0x0202, &mut data), 0, "WSAStartup failed");
    }

    if mode == "respawn" {
        wait_for_pid_to_exit(after_pid);
        let listener = bind_until_free(port);
        println!("RESPAWNED={}", listener.local_addr().unwrap().port());
        let _ = std::io::stdout().flush();
        hold(vec![listener]);
    }

    if mode == "time-wait" {
        time_wait_mode(port);
    }

    let primary = if dual {
        bind_v6(port, reuse, dual)
    } else {
        bind_v4(port, reuse)
    };
    let primary_port = primary.local_addr().unwrap().port();
    println!("PORT={primary_port}");
    let _ = std::io::stdout().flush();

    let mut listeners = vec![primary];
    for _ in 0..extra {
        let listener = bind_v4(0, false);
        println!("EXTRA_PORT={}", listener.local_addr().unwrap().port());
        let _ = std::io::stdout().flush();
        listeners.push(listener);
    }
    hold(listeners);
}

fn hold(listeners: Vec<TcpListener>) {
    loop {
        thread::sleep(Duration::from_secs(3_600));
        // Keep the sockets alive; accepting is unnecessary for these tests.
        std::hint::black_box(&listeners);
    }
}

/// Accept one connection, close it first (the server-side socket becomes the
/// active closer), wait for the client to drop, then close the listener. The
/// only remaining row on the port should be TIME_WAIT.
fn time_wait_mode(port: u16) {
    let listener = bind_v4(port, false);
    let port = listener.local_addr().unwrap().port();
    println!("PORT={port}");
    let _ = std::io::stdout().flush();

    let (stream, _) = listener.accept().expect("accept time-wait client");
    drop(stream);
    println!("CLOSED=1");
    let _ = std::io::stdout().flush();

    // Give the peer time to observe our FIN and close its side, moving our
    // accepted socket through TIME_WAIT.
    thread::sleep(Duration::from_millis(500));
    drop(listener);
    println!("DONE=1");
    let _ = std::io::stdout().flush();

    loop {
        thread::sleep(Duration::from_secs(3_600));
    }
}

fn bind_until_free(port: u16) -> TcpListener {
    for _ in 0..500 {
        if let Ok(listener) = std::panic::catch_unwind(|| bind_v4(port, true)) {
            return listener;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("could not rebind port {port}");
}

fn wait_for_pid_to_exit(pid: u32) {
    if pid == 0 {
        return;
    }
    loop {
        if !pid_alive(pid) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn pid_alive(pid: u32) -> bool {
    let handle: HANDLE = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return false;
    }
    let status = unsafe { WaitForSingleObject(handle, 0) };
    unsafe { CloseHandle(handle) };
    status == WAIT_TIMEOUT
}

fn bind_v4(port: u16, reuse: bool) -> TcpListener {
    unsafe {
        let socket = socket(AF_INET as i32, SOCK_STREAM, IPPROTO_TCP);
        assert!(socket != usize::MAX, "socket failed");
        if reuse {
            set_option(socket, SOL_SOCKET, SO_REUSEADDR, 1);
        }
        let address = SOCKADDR_IN {
            sin_family: AF_INET,
            sin_port: port.to_be(),
            sin_addr: IN_ADDR {
                S_un: windows_sys::Win32::Networking::WinSock::IN_ADDR_0 { S_addr: 0 },
            },
            ..SOCKADDR_IN::default()
        };
        let result = bind(
            socket,
            &address as *const SOCKADDR_IN as *const _,
            std::mem::size_of::<SOCKADDR_IN>() as i32,
        );
        assert_eq!(result, 0, "bind v4 {port} failed: {}", last_error());
        listen(socket, 64);
        TcpListener::from_raw_socket(socket as u64)
    }
}

fn bind_v6(port: u16, reuse: bool, dual: bool) -> TcpListener {
    unsafe {
        let socket = socket(AF_INET6 as i32, SOCK_STREAM, IPPROTO_TCP);
        assert!(socket != usize::MAX, "socket v6 failed");
        if reuse {
            set_option(socket, SOL_SOCKET, SO_REUSEADDR, 1);
        }
        if dual {
            set_option(socket, IPPROTO_IPV6, IPV6_V6ONLY, 0);
        }
        let address = SOCKADDR_IN6 {
            sin6_family: AF_INET6,
            sin6_port: port.to_be(),
            sin6_addr: IN6_ADDR {
                u: windows_sys::Win32::Networking::WinSock::IN6_ADDR_0 { Byte: [0; 16] },
            },
            ..SOCKADDR_IN6::default()
        };
        let result = bind(
            socket,
            &address as *const SOCKADDR_IN6 as *const _,
            std::mem::size_of::<SOCKADDR_IN6>() as i32,
        );
        assert_eq!(result, 0, "bind v6 {port} failed: {}", last_error());
        listen(socket, 64);
        TcpListener::from_raw_socket(socket as u64)
    }
}

unsafe fn set_option(socket: usize, level: i32, name: i32, value: i32) {
    let result = unsafe {
        setsockopt(
            socket,
            level,
            name,
            &value as *const i32 as *const u8,
            std::mem::size_of::<i32>() as i32,
        )
    };
    assert_eq!(result, 0, "setsockopt {name} failed: {}", last_error());
}

fn last_error() -> i32 {
    unsafe { windows_sys::Win32::Networking::WinSock::WSAGetLastError() }
}
