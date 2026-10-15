#![cfg(windows)]

use std::{
    process::{Command, Output},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn cli(args: &[&str]) -> Output {
    let binary = std::env::var_os("NSSM_RS_TEST_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_nssm-rs").into());
    Command::new(binary)
        .args(args)
        .output()
        .expect("nssm-rs starts")
}

fn service_name() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .as_nanos();
    format!("NssmRSmoke{}_{}", std::process::id(), stamp)
}

struct ServiceCleanup(String);

impl Drop for ServiceCleanup {
    fn drop(&mut self) {
        let _ = cli(&["stop", &self.0]);
        let _ = cli(&["remove", &self.0, "confirm"]);
    }
}

fn wait_for_state(service: &str, expected: &str) -> bool {
    for _ in 0..100 {
        let status = cli(&["status", service]);
        if status.status.success() && String::from_utf8_lossy(&status.stdout).trim() == expected {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn service_status_snapshot(
    service: &str,
) -> Option<windows_sys::Win32::System::Services::SERVICE_STATUS> {
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_QUERY_STATUS,
    };

    let manager = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return None;
    }
    let name: Vec<u16> = service.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS) };
    if handle.is_null() {
        unsafe { CloseServiceHandle(manager) };
        return None;
    }
    let mut status = std::mem::MaybeUninit::uninit();
    let queried = unsafe { QueryServiceStatus(handle, status.as_mut_ptr()) } != 0;
    unsafe {
        CloseServiceHandle(handle);
        CloseServiceHandle(manager);
    }
    queried.then(|| unsafe { status.assume_init() })
}

#[test]
fn installs_starts_reports_stops_and_removes_a_simple_real_service() {
    let service = service_name();
    let _cleanup = ServiceCleanup(service.clone());
    let ping =
        std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()) + "\\System32\\PING.EXE";

    let installed = cli(&["install", &service, &ping, "-t", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert_eq!(
        installed.stdout,
        format!("Service \"{service}\" installed successfully!\r\r\n").as_bytes()
    );
    assert!(installed.stderr.is_empty());
    let application = cli(&["get", &service, "Application"]);
    assert!(application.status.success());
    assert_eq!(String::from_utf8_lossy(&application.stdout).trim(), ping);

    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(
        wait_for_state(&service, "SERVICE_RUNNING"),
        "service never reached SERVICE_RUNNING"
    );
    let service_status =
        service_status_snapshot(&service).expect("SCM returns the running service status");
    assert_ne!(
        service_status.dwControlsAccepted
            & windows_sys::Win32::System::Services::SERVICE_ACCEPT_POWEREVENT,
        0,
        "running service registers for power broadcast controls"
    );
    assert_eq!(
        service_status.dwServiceType,
        windows_sys::Win32::System::Services::SERVICE_WIN32_OWN_PROCESS,
        "SCM reports the configured own-process service type"
    );
    let status_code = cli(&["statuscode", &service]);
    assert_eq!(status_code.status.code(), Some(4));
    assert_eq!(
        String::from_utf8_lossy(&status_code.stdout).trim(),
        "SERVICE_RUNNING"
    );

    let processes = cli(&["processes", &service]);
    assert!(processes.status.success());
    assert!(
        String::from_utf8_lossy(&processes.stdout)
            .to_ascii_uppercase()
            .contains("PING.EXE"),
        "process listing omitted the test child: {}",
        String::from_utf8_lossy(&processes.stdout)
    );

    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(
        wait_for_state(&service, "SERVICE_STOPPED"),
        "service never reached SERVICE_STOPPED"
    );

    let removed = cli(&["remove", &service, "confirm"]);
    assert!(
        removed.status.success(),
        "remove failed: {}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert_eq!(
        removed.stdout,
        format!("Service \"{service}\" removed successfully!\r\r\n").as_bytes()
    );
    let mut disappeared = false;
    for _ in 0..100 {
        if !cli(&["status", &service]).status.success() {
            disappeared = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(disappeared, "service remained queryable after removal");
}
