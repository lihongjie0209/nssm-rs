#![cfg(windows)]

use std::{
    io::Write,
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn hidden_powershell(powershell: &str) -> Command {
    let mut command = Command::new(powershell);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

static SERVICE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const HOOK_ABI_VARIABLES: [&str; 22] = [
    "NSSM_HOOK_VERSION",
    "NSSM_EVENT",
    "NSSM_ACTION",
    "NSSM_TRIGGER",
    "NSSM_LAST_CONTROL",
    "NSSM_EXE",
    "NSSM_CONFIGURATION",
    "NSSM_VERSION",
    "NSSM_BUILD_DATE",
    "NSSM_PID",
    "NSSM_RUNTIME",
    "NSSM_APPLICATION_PID",
    "NSSM_APPLICATION_RUNTIME",
    "NSSM_EXITCODE",
    "NSSM_DEADLINE",
    "NSSM_SERVICE_NAME",
    "NSSM_SERVICE_DISPLAYNAME",
    "NSSM_START_REQUESTED_COUNT",
    "NSSM_START_COUNT",
    "NSSM_EXIT_COUNT",
    "NSSM_THROTTLE_COUNT",
    "NSSM_COMMAND_LINE",
];

fn read_hook_environment(path: &str) -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(path)
        .expect("hook ABI environment was captured")
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_ascii_uppercase(), value.trim().to_owned()))
        .collect()
}

fn assert_hook_abi_present(environment: &std::collections::HashMap<String, String>, hook: &str) {
    for variable in HOOK_ABI_VARIABLES {
        assert!(
            environment.contains_key(variable),
            "{hook} hook ABI omitted {variable}"
        );
    }
}

fn cli(args: &[&str]) -> Output {
    let binary = std::env::var_os("NSSM_RS_TEST_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_nssm-rs").into());
    Command::new(binary)
        .args(args)
        .output()
        .expect("nssm-rs executable starts")
}

fn windows_cli_bytes(value: &str) -> Vec<u8> {
    use windows_sys::Win32::{
        Globalization::WideCharToMultiByte,
        System::{Console::GetConsoleWindow, Threading::GetCurrentProcessId},
        UI::WindowsAndMessaging::GetWindowThreadProcessId,
    };

    let console = unsafe { GetConsoleWindow() };
    let mut console_process = 0;
    let utf8 = !console.is_null()
        && unsafe { GetWindowThreadProcessId(console, &mut console_process) } != 0
        && unsafe { GetCurrentProcessId() != console_process };
    let (code_page, flags) = if utf8 {
        (65001, 0)
    } else {
        (1252, 0x0000_0400)
    };

    let wide: Vec<u16> = value.encode_utf16().collect();
    let required = unsafe {
        WideCharToMultiByte(
            code_page,
            flags,
            wide.as_ptr(),
            wide.len() as i32,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    assert!(required >= 0);
    let mut bytes = vec![0; required as usize];
    let written = unsafe {
        WideCharToMultiByte(
            code_page,
            flags,
            wide.as_ptr(),
            wide.len() as i32,
            bytes.as_mut_ptr(),
            required,
            std::ptr::null(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(written, required);
    bytes
}

fn cli_stdout_value(output: &Output) -> &[u8] {
    output
        .stdout
        .strip_suffix(b"\r\n")
        .unwrap_or(&output.stdout)
}

fn cli_with_input(args: &[&str], input: &str) -> Output {
    let binary = std::env::var_os("NSSM_RS_TEST_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_nssm-rs").into());
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("nssm-rs starts with piped input");
    child
        .stdin
        .take()
        .expect("child stdin is piped")
        .write_all(input.as_bytes())
        .expect("confirmation is written");
    child.wait_with_output().expect("nssm-rs exits")
}

fn recovery_flag_includes_non_crash_failures(service: &str) -> bool {
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceConfig2W, SC_MANAGER_CONNECT,
        SERVICE_CONFIG_FAILURE_ACTIONS_FLAG, SERVICE_FAILURE_ACTIONS_FLAG, SERVICE_QUERY_CONFIG,
    };

    let manager = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return false;
    }
    let name: Vec<u16> = service.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe { OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_CONFIG) };
    if handle.is_null() {
        unsafe { CloseServiceHandle(manager) };
        return false;
    }
    let mut needed = 0;
    unsafe {
        QueryServiceConfig2W(
            handle,
            SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
    }
    let mut storage = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    let enabled = !storage.is_empty()
        && unsafe {
            QueryServiceConfig2W(
                handle,
                SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
                storage.as_mut_ptr().cast(),
                (storage.len() * std::mem::size_of::<usize>()) as u32,
                &mut needed,
            )
        } != 0
        && unsafe { *(storage.as_ptr().cast::<SERVICE_FAILURE_ACTIONS_FLAG>()) }
            .fFailureActionsOnNonCrashFailures
            != 0;
    unsafe {
        CloseServiceHandle(handle);
        CloseServiceHandle(manager);
    }
    enabled
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

fn wait_for_service_state(service: &str, expected: &str) -> bool {
    for _ in 0..100 {
        let status = cli(&["status", service]);
        if status.status.success() && String::from_utf8_lossy(&status.stdout).trim() == expected {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

fn wait_for_windows_powershell_condition(script: &str) -> bool {
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if hidden_powershell(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .status()
            .is_ok_and(|status| status.success())
        {
            return true;
        }
        thread::sleep(Duration::from_millis(250));
    }
    false
}

fn file_identity(path: &str) -> (u32, u64) {
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let file = std::fs::File::open(path).expect("log file opens for identity query");
    let mut information = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    assert_ne!(
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, information.as_mut_ptr()) },
        0,
        "Windows returns log file identity"
    );
    let information = unsafe { information.assume_init() };
    (
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    )
}

fn unique_service_name() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = SERVICE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("NssmRTest{}_{}_{}", std::process::id(), stamp, sequence)
}

struct Cleanup(String);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = cli(&["stop", &self.0]);
        let _ = cli(&["remove", &self.0, "confirm"]);
    }
}

struct ProcessCleanup(u32);
impl Drop for ProcessCleanup {
    fn drop(&mut self) {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess},
        };
        let process = unsafe { OpenProcess(PROCESS_TERMINATE, 0, self.0) };
        if !process.is_null() {
            unsafe {
                TerminateProcess(process, 0);
                CloseHandle(process);
            }
        }
    }
}

struct RestoreStopMethods(String);
impl Drop for RestoreStopMethods {
    fn drop(&mut self) {
        let _ = cli(&["set", &self.0, "AppStopMethodSkip", "0"]);
    }
}

struct NativeServiceCleanup(String);
impl Drop for NativeServiceCleanup {
    fn drop(&mut self) {
        let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let _ = Command::new(format!("{windows}\\System32\\sc.exe"))
            .args(["delete", &self.0])
            .output();
    }
}

#[test]
fn dump_exports_native_service_metadata() {
    let service = unique_service_name();
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let sc = format!("{windows}\\System32\\sc.exe");
    let created = Command::new(&sc)
        .args([
            "create",
            &service,
            "binPath=",
            "C:\\Windows\\System32\\cmd.exe /c exit",
            "start=",
            "demand",
            "type=",
            "own",
        ])
        .output()
        .expect("sc.exe starts to create a native service");
    assert!(
        created.status.success(),
        "native service creation failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let _cleanup = NativeServiceCleanup(service.clone());

    let dumped = cli(&["dump", &service]);
    assert!(
        dumped.status.success(),
        "native dump failed: {}",
        String::from_utf8_lossy(&dumped.stderr)
    );
    let output = String::from_utf8_lossy(&dumped.stdout);
    assert!(output.contains(&format!("install {service}")), "{output}");
    assert!(
        output.contains(&format!("set {service} Start SERVICE_DEMAND_START")),
        "{output}"
    );
    assert!(
        output.contains(&format!("set {service} Type SERVICE_WIN32_OWN_PROCESS")),
        "{output}"
    );
}

#[test]
fn installs_runs_stops_and_removes_a_real_scm_service() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let ping =
        std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()) + "\\System32\\PING.EXE";

    let missing_status = cli(&["statuscode", &service]);
    assert_eq!(missing_status.status.code(), Some(0));
    assert!(missing_status.stdout.is_empty());
    assert!(!missing_status.stderr.is_empty());

    let installed = cli(&["install", &service, &ping, "-t", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        recovery_flag_includes_non_crash_failures(&service),
        "install enables SCM recovery actions for non-crash service failures"
    );

    let exit_policy_before_start = cli(&["get", &service, "AppExit", "Default"]);
    assert!(exit_policy_before_start.status.success());
    assert_eq!(
        String::from_utf8_lossy(&exit_policy_before_start.stdout).trim(),
        "Restart"
    );

    assert!(
        cli(&["set", &service, "Description", "nssm-rs test service"])
            .status
            .success()
    );
    let description = cli(&["get", &service, "Description"]);
    assert!(description.status.success());
    assert!(description.stdout.ends_with(b"\r\n"));
    assert_eq!(
        String::from_utf8_lossy(&description.stdout).trim(),
        "nssm-rs test service"
    );
    assert!(cli(&["reset", &service, "Description"]).status.success());

    let object_name = cli(&["set", &service, "ObjectName", "SYSTEM"]);
    assert!(
        object_name.status.success(),
        "setting the built-in service account failed: {}",
        String::from_utf8_lossy(&object_name.stderr)
    );
    let object_name = cli(&["get", &service, "ObjectName"]);
    assert!(object_name.status.success());
    assert_eq!(
        String::from_utf8_lossy(&object_name.stdout).trim(),
        "LocalSystem"
    );

    let start_type = cli(&["set", &service, "Start", "SERVICE_DEMAND_START"]);
    assert!(
        start_type.status.success(),
        "setting demand start failed: {}",
        String::from_utf8_lossy(&start_type.stderr)
    );
    let start_type = cli(&["get", &service, "Start"]);
    assert!(start_type.status.success());
    assert_eq!(
        String::from_utf8_lossy(&start_type.stdout).trim(),
        "SERVICE_DEMAND_START"
    );
    assert!(
        cli(&["set", &service, "Start", "SERVICE_AUTO_START"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "Start", "SERVICE_DELAYED_AUTO_START"])
            .status
            .success(),
        "setting delayed automatic start succeeds"
    );
    let start_type = cli(&["get", &service, "Start"]);
    assert!(start_type.status.success());
    assert_eq!(
        String::from_utf8_lossy(&start_type.stdout).trim(),
        "SERVICE_DELAYED_AUTO_START"
    );
    assert!(
        cli(&["set", &service, "Start", "SERVICE_AUTO_START"])
            .status
            .success()
    );
    let start_type = cli(&["get", &service, "Start"]);
    assert!(start_type.status.success());
    assert_eq!(
        String::from_utf8_lossy(&start_type.stdout).trim(),
        "SERVICE_AUTO_START"
    );

    let invalid_start = cli(&["set", &service, "Start", "NOT_A_START_TYPE"]);
    assert_eq!(invalid_start.status.code(), Some(6));
    assert!(invalid_start.stdout.is_empty());
    let unknown_setting = cli(&["get", &service, "NssmRUnknownSetting"]);
    assert_eq!(unknown_setting.status.code(), Some(1));
    assert!(unknown_setting.stdout.is_empty());

    assert!(
        cli(&["set", &service, "DisplayName", "NSSM Rust Test"])
            .status
            .success()
    );
    let display_name = cli(&["get", &service, "DisplayName"]);
    assert!(display_name.status.success());
    assert_eq!(
        String::from_utf8_lossy(&display_name.stdout).trim(),
        "NSSM Rust Test"
    );
    assert!(cli(&["reset", &service, "DisplayName"]).status.success());

    assert!(
        cli(&["set", &service, "Environment", "+NSSMR_NATIVE=value"])
            .status
            .success()
    );
    let native_environment = cli(&["get", &service, "Environment", "NSSMR_NATIVE"]);
    assert!(native_environment.status.success());
    assert_eq!(
        String::from_utf8_lossy(&native_environment.stdout).trim(),
        "value"
    );
    assert!(cli(&["reset", &service, "Environment"]).status.success());

    let dependency = cli(&["set", &service, "DependOnService", "RpcSs"]);
    assert!(
        dependency.status.success(),
        "dependency set failed: {}",
        String::from_utf8_lossy(&dependency.stderr)
    );
    let dependency = cli(&["get", &service, "DependOnService"]);
    assert!(dependency.status.success());
    assert_eq!(String::from_utf8_lossy(&dependency.stdout).trim(), "RpcSs");
    assert!(
        cli(&["set", &service, "DependOnService", "+LanmanWorkstation"])
            .status
            .success()
    );
    let dependency = cli(&["get", &service, "DependOnService"]);
    assert_eq!(dependency.stdout, b"RpcSs\r\r\nLanmanWorkstation\r\n");
    assert_eq!(
        String::from_utf8_lossy(&dependency.stdout).lines().count(),
        2
    );
    assert!(
        cli(&["set", &service, "DependOnService", "-RpcSs"])
            .status
            .success()
    );
    let dependency = cli(&["get", &service, "DependOnService"]);
    assert_eq!(
        String::from_utf8_lossy(&dependency.stdout).trim(),
        "LanmanWorkstation"
    );
    let group_dependency = cli(&["set", &service, "DependOnGroup", "+NetworkProvider"]);
    assert!(
        group_dependency.status.success(),
        "group dependency set failed: {}",
        String::from_utf8_lossy(&group_dependency.stderr)
    );
    let dependency = cli(&["reset", &service, "DependOnService"]);
    assert!(dependency.status.success());
    let dependency_after_reset = cli(&["get", &service, "DependOnService"]);
    assert_eq!(
        String::from_utf8_lossy(&dependency_after_reset.stdout).trim(),
        "",
        "reset clears service dependencies"
    );
    let group_dependency = cli(&["get", &service, "DependOnGroup"]);
    assert!(group_dependency.status.success());
    assert_eq!(
        String::from_utf8_lossy(&group_dependency.stdout).trim(),
        "+NetworkProvider"
    );
    assert!(cli(&["reset", &service, "DependOnGroup"]).status.success());
    let group_dependency_after_reset = cli(&["get", &service, "DependOnGroup"]);
    assert_eq!(
        String::from_utf8_lossy(&group_dependency_after_reset.stdout).trim(),
        "+NetworkProvider",
        "NSSM reset leaves load-order group dependencies unchanged"
    );

    let managed = cli(&["list"]);
    assert!(managed.status.success());
    assert!(
        String::from_utf8_lossy(&managed.stdout)
            .lines()
            .any(|line| line == service)
    );
    let all = cli(&["list", "all", "ignored"]);
    assert!(all.status.success());
    assert!(
        String::from_utf8_lossy(&all.stdout)
            .lines()
            .any(|line| line == service)
    );

    let code_action = cli(&["set", &service, "AppExit", "23", "Ignore"]);
    assert!(
        code_action.status.success(),
        "setting exit-code action failed: {}",
        String::from_utf8_lossy(&code_action.stderr)
    );
    let dump = cli(&["dump", &service, "NssmR Dump Copy"]);
    assert!(
        dump.status.success(),
        "dump failed: {}",
        String::from_utf8_lossy(&dump.stderr)
    );
    let dump = String::from_utf8_lossy(&dump.stdout);
    assert!(
        dump.lines()
            .next()
            .is_some_and(|line| line.contains(" install \"NssmR Dump Copy\" "))
    );
    assert!(dump.contains("set \"NssmR Dump Copy\" AppParameters \"-t 127.0.0.1\""));
    assert!(dump.contains("set \"NssmR Dump Copy\" AppExit Default Restart"));
    assert_eq!(
        dump.matches("set \"NssmR Dump Copy\" AppExit Default Restart")
            .count(),
        2,
        "NSSM dumps the default action once per AppExit registry value"
    );
    assert!(
        dump.to_ascii_lowercase()
            .contains("set \"nssmr dump copy\" appdirectory c:\\windows\\system32")
    );
    assert!(dump.contains("set \"NssmR Dump Copy\" DisplayName \"NssmR Dump Copy\""));
    assert!(dump.contains("set \"NssmR Dump Copy\" ObjectName LocalSystem"));
    assert!(dump.contains("set \"NssmR Dump Copy\" Type SERVICE_WIN32_OWN_PROCESS"));
    let copied_service = "NssmR Dump Copy";
    let _copy_cleanup = Cleanup(copied_service.to_owned());
    for line in dump.lines() {
        let parsed = nssm_rs::split_windows_args(line);
        assert!(
            parsed.len() >= 2,
            "dump emitted an incomplete command: {line}"
        );
        let arguments = parsed
            .iter()
            .skip(1)
            .map(String::as_str)
            .collect::<Vec<_>>();
        let replayed = cli(&arguments);
        assert!(
            replayed.status.success(),
            "dump command failed: {line}\nstderr: {}",
            String::from_utf8_lossy(&replayed.stderr)
        );
    }
    for (setting, expected) in [
        ("Application", ping.as_str()),
        ("AppParameters", "-t 127.0.0.1"),
        ("Type", "SERVICE_WIN32_OWN_PROCESS"),
    ] {
        let copied_value = cli(&["get", copied_service, setting]);
        assert!(
            copied_value.status.success(),
            "dumped {setting} could not be read back: {}",
            String::from_utf8_lossy(&copied_value.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&copied_value.stdout).trim(),
            expected,
            "dump did not preserve {setting}"
        );
    }

    let set_delay = cli(&["set", &service, "AppRestartDelay", "250"]);
    assert!(
        set_delay.status.success(),
        "setting write failed: {}",
        String::from_utf8_lossy(&set_delay.stderr)
    );
    let delay = cli(&["get", &service, "AppRestartDelay"]);
    assert!(delay.status.success());
    assert_eq!(String::from_utf8_lossy(&delay.stdout).trim(), "250");
    let reset_delay = cli(&["reset", &service, "AppRestartDelay"]);
    assert!(reset_delay.status.success());
    let delay = cli(&["get", &service, "AppRestartDelay"]);
    assert_eq!(String::from_utf8_lossy(&delay.stdout).trim(), "0");

    assert!(
        cli(&["set", &service, "AppKillProcessTree", "0"])
            .status
            .success()
    );
    let kill_tree = cli(&["get", &service, "AppKillProcessTree"]);
    assert_eq!(String::from_utf8_lossy(&kill_tree.stdout).trim(), "0");
    assert!(
        cli(&["reset", &service, "AppKillProcessTree"])
            .status
            .success()
    );
    let kill_tree = cli(&["get", &service, "AppKillProcessTree"]);
    assert_eq!(String::from_utf8_lossy(&kill_tree.stdout).trim(), "1");

    assert!(
        cli(&["set", &service, "AppPriority", "HIGH_PRIORITY_CLASS"])
            .status
            .success()
    );
    let priority = cli(&["get", &service, "AppPriority"]);
    assert_eq!(
        String::from_utf8_lossy(&priority.stdout).trim(),
        "HIGH_PRIORITY_CLASS"
    );
    assert!(cli(&["reset", &service, "AppPriority"]).status.success());
    let affinity_set = cli(&["set", &service, "AppAffinity", "0"]);
    assert!(
        affinity_set.status.success(),
        "affinity write failed: {}",
        String::from_utf8_lossy(&affinity_set.stderr)
    );
    let affinity = cli(&["get", &service, "AppAffinity"]);
    assert_eq!(String::from_utf8_lossy(&affinity.stdout).trim(), "0");
    assert!(cli(&["reset", &service, "AppAffinity"]).status.success());
    let affinity = cli(&["get", &service, "AppAffinity"]);
    assert_eq!(String::from_utf8_lossy(&affinity.stdout).trim(), "All");
    for (input, expected) in [("0-1", "0,1"), ("0,1,2", "0-2")] {
        let configured = cli(&["set", &service, "AppAffinity", input]);
        assert!(
            configured.status.success(),
            "affinity {input} should be accepted: {}",
            String::from_utf8_lossy(&configured.stderr)
        );
        let affinity = cli(&["get", &service, "AppAffinity"]);
        assert_eq!(String::from_utf8_lossy(&affinity.stdout).trim(), expected);
    }
    assert!(
        cli(&["set", &service, "AppAffinity", "0-64"])
            .status
            .success()
    );
    let affinity = cli(&["get", &service, "AppAffinity"]);
    assert_eq!(String::from_utf8_lossy(&affinity.stdout).trim(), "0-63");
    let invalid_cpu = cli(&["set", &service, "AppAffinity", "64"]);
    assert_eq!(
        invalid_cpu.status.code(),
        Some(6),
        "NSSM returns status 6 for an invalid affinity value"
    );
    let affinity = cli(&["get", &service, "AppAffinity"]);
    assert_eq!(String::from_utf8_lossy(&affinity.stdout).trim(), "0-63");
    for default in ["All", "Default", "*"] {
        assert!(
            cli(&["set", &service, "AppAffinity", default])
                .status
                .success()
        );
        let affinity = cli(&["get", &service, "AppAffinity"]);
        assert_eq!(String::from_utf8_lossy(&affinity.stdout).trim(), "All");
        let dumped = cli(&["dump", &service, "AffinityDefaultCopy"]);
        assert!(dumped.status.success());
        assert!(!String::from_utf8_lossy(&dumped.stdout).contains("AppAffinity"));
    }

    let set_exit = cli(&["set", &service, "AppExit", "23", "Ignore"]);
    assert!(
        set_exit.status.success(),
        "AppExit write failed: {}",
        String::from_utf8_lossy(&set_exit.stderr)
    );
    let exit = cli(&["get", &service, "AppExit", "23"]);
    assert!(exit.status.success());
    assert_eq!(String::from_utf8_lossy(&exit.stdout).trim(), "Ignore");
    let fallback = cli(&["get", &service, "AppExit", "24"]);
    assert_eq!(String::from_utf8_lossy(&fallback.stdout).trim(), "Restart");
    assert!(cli(&["reset", &service, "AppExit", "23"]).status.success());
    let exit = cli(&["get", &service, "AppExit", "23"]);
    assert_eq!(String::from_utf8_lossy(&exit.stdout).trim(), "Restart");

    let set_env = cli(&["set", &service, "AppEnvironment", "+NSSMR_TEST=value"]);
    assert!(
        set_env.status.success(),
        "environment write failed: {}",
        String::from_utf8_lossy(&set_env.stderr)
    );
    let env = cli(&["get", &service, "AppEnvironment", "NSSMR_TEST"]);
    assert!(env.status.success());
    assert_eq!(String::from_utf8_lossy(&env.stdout).trim(), "value");

    let set_hook = cli(&[
        "set",
        &service,
        "AppEvents",
        "Start/Pre",
        "cmd.exe /c exit 0",
    ]);
    assert!(
        set_hook.status.success(),
        "hook write failed: {}",
        String::from_utf8_lossy(&set_hook.stderr)
    );
    let hook = cli(&["get", &service, "AppEvents", "Start/Pre"]);
    assert_eq!(
        String::from_utf8_lossy(&hook.stdout).trim(),
        "cmd.exe /c exit 0"
    );
    assert!(
        cli(&["reset", &service, "AppEvents", "Start/Pre"])
            .status
            .success()
    );

    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );

    let mut running = false;
    for _ in 0..30 {
        let status = cli(&["status", &service]);
        if status.status.success()
            && String::from_utf8_lossy(&status.stdout).trim() == "SERVICE_RUNNING"
        {
            running = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(running, "SCM never reported the wrapper as running");
    let escaped_service = service.replace('\'', "''");
    let start_control_event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1040}} -MaxEvents 500 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 2 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq 'START') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&start_control_event),
        "Application log contains event 1040 for the handled START control"
    );

    let default_exit_policy = cli(&["get", &service, "AppExit", "Default"]);
    assert!(default_exit_policy.status.success());
    assert_eq!(
        String::from_utf8_lossy(&default_exit_policy.stdout).trim(),
        "Restart"
    );

    let status_code = cli(&["statuscode", &service]);
    assert_eq!(status_code.status.code(), Some(4));
    assert_eq!(
        String::from_utf8_lossy(&status_code.stdout).trim(),
        "SERVICE_RUNNING"
    );

    let tree = cli(&["processes", &service]);
    assert!(
        tree.status.success(),
        "process enumeration failed: {}",
        String::from_utf8_lossy(&tree.stderr)
    );
    let tree = String::from_utf8_lossy(&tree.stdout);
    assert!(
        tree.lines().count() >= 2,
        "process output did not include the wrapped child: {tree}"
    );
    assert!(
        tree.to_ascii_uppercase().contains("PING.EXE"),
        "wrapped process path was not reported: {tree}"
    );

    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let stop_control_event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1040}} -MaxEvents 500 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 2 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq 'STOP') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&stop_control_event),
        "Application log contains event 1040 for the handled STOP control"
    );
    let status = cli(&["status", &service]);
    assert!(status.status.success());
    assert_eq!(
        String::from_utf8_lossy(&status.stdout).trim(),
        "SERVICE_STOPPED"
    );
    let status_code = cli(&["statuscode", &service]);
    assert_eq!(status_code.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&status_code.stdout).trim(),
        "SERVICE_STOPPED"
    );

    let removed = cli(&["remove", &service, "confirm"]);
    assert!(
        removed.status.success(),
        "remove failed: {}",
        String::from_utf8_lossy(&removed.stderr)
    );
}

#[test]
fn all_dword_settings_round_trip_and_reset_to_nssm_defaults() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let ping =
        std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()) + "\\System32\\PING.EXE";
    let installed = cli(&["install", &service, &ping, "-t", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );

    let settings = [
        ("AppNoConsole", "1", "1", "0"),
        (
            "AppPriority",
            "HIGH_PRIORITY_CLASS",
            "HIGH_PRIORITY_CLASS",
            "NORMAL_PRIORITY_CLASS",
        ),
        ("AppRestartDelay", "321", "321", "0"),
        ("AppStdinShareMode", "3", "3", "2"),
        ("AppStdinCreationDisposition", "2", "2", "3"),
        ("AppStdinFlagsAndAttributes", "256", "256", "128"),
        ("AppStdoutShareMode", "2", "2", "3"),
        ("AppStdoutCreationDisposition", "3", "3", "4"),
        ("AppStdoutFlagsAndAttributes", "256", "256", "128"),
        ("AppStdoutCopyAndTruncate", "1", "1", "0"),
        ("AppStderrShareMode", "2", "2", "3"),
        ("AppStderrCreationDisposition", "3", "3", "4"),
        ("AppStderrFlagsAndAttributes", "256", "256", "128"),
        ("AppStderrCopyAndTruncate", "1", "1", "0"),
        ("AppStopMethodSkip", "7", "7", "0"),
        ("AppStopMethodConsole", "321", "321", "1500"),
        ("AppStopMethodWindow", "321", "321", "1500"),
        ("AppStopMethodThreads", "321", "321", "1500"),
        ("AppKillProcessTree", "0", "0", "1"),
        ("AppThrottle", "321", "321", "1500"),
        ("AppRedirectHook", "1", "1", "0"),
        ("AppRotateFiles", "1", "1", "0"),
        ("AppRotateOnline", "1", "1", "0"),
        ("AppRotateSeconds", "321", "321", "0"),
        ("AppRotateBytes", "1234", "1234", "0"),
        ("AppRotateBytesHigh", "2", "2", "0"),
        ("AppRotateDelay", "321", "321", "0"),
        ("AppTimestampLog", "1", "1", "0"),
    ];

    for (setting, value, expected_value, expected_default) in settings {
        let configured = cli(&["set", &service, setting, value]);
        assert!(
            configured.status.success(),
            "set {setting} failed: {}",
            String::from_utf8_lossy(&configured.stderr)
        );
        let queried = cli(&["get", &service, setting]);
        assert!(
            queried.status.success(),
            "get {setting} failed: {}",
            String::from_utf8_lossy(&queried.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&queried.stdout).trim(),
            expected_value,
            "{setting} did not read back its configured value"
        );

        let reset = cli(&["reset", &service, setting]);
        assert!(
            reset.status.success(),
            "reset {setting} failed: {}",
            String::from_utf8_lossy(&reset.stderr)
        );
        let queried = cli(&["get", &service, setting]);
        assert!(
            queried.status.success(),
            "get default {setting} failed: {}",
            String::from_utf8_lossy(&queried.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&queried.stdout).trim(),
            expected_default,
            "reset {setting} did not restore the NSSM default"
        );
    }
}

#[test]
fn string_settings_round_trip_unicode_spaces_and_reset_values() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let ping = format!("{windows}\\System32\\PING.EXE");
    let installed = cli(&["install", &service, &ping, "-t", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );

    let temporary = std::env::temp_dir();
    let settings = [
        ("Application", format!("{windows}\\System32\\cmd.exe"), ""),
        ("AppParameters", "--label \"two words\"".to_owned(), ""),
        (
            "AppDirectory",
            "C:\\Program Files\\服务 测试".to_owned(),
            "",
        ),
        (
            "AppStdin",
            temporary
                .join(format!("{service} standard input.txt"))
                .to_string_lossy()
                .into_owned(),
            "",
        ),
        (
            "AppStdout",
            temporary
                .join(format!("{service} standard output.txt"))
                .to_string_lossy()
                .into_owned(),
            "",
        ),
        (
            "AppStderr",
            temporary
                .join(format!("{service} standard error.txt"))
                .to_string_lossy()
                .into_owned(),
            "",
        ),
    ];

    for (setting, value, expected_default) in settings {
        let configured = cli(&["set", &service, setting, &value]);
        assert!(
            configured.status.success(),
            "set {setting} failed: {}",
            String::from_utf8_lossy(&configured.stderr)
        );
        let queried = cli(&["get", &service, setting]);
        assert!(
            queried.status.success(),
            "get {setting} failed: {}",
            String::from_utf8_lossy(&queried.stderr)
        );
        assert_eq!(
            cli_stdout_value(&queried),
            windows_cli_bytes(&value),
            "{setting} did not preserve its string value in CLI output"
        );

        let reset = cli(&["reset", &service, setting]);
        assert!(
            reset.status.success(),
            "reset {setting} failed: {}",
            String::from_utf8_lossy(&reset.stderr)
        );
        let queried = cli(&["get", &service, setting]);
        assert!(
            queried.status.success(),
            "get reset {setting} failed: {}",
            String::from_utf8_lossy(&queried.stderr)
        );
        assert_eq!(
            cli_stdout_value(&queried),
            windows_cli_bytes(expected_default),
            "reset {setting} did not restore its default"
        );
        if setting == "Application" {
            let other_setting = cli(&["get", &service, "AppRotateBytes"]);
            assert!(
                other_setting.status.success(),
                "resetting Application changed service classification: {}",
                String::from_utf8_lossy(&other_setting.stderr)
            );
        }
    }
}

#[test]
fn missing_service_commands_return_nssm_service_lookup_status() {
    let service = unique_service_name();
    for arguments in [
        vec!["status", service.as_str()],
        vec!["start", service.as_str()],
        vec!["stop", service.as_str()],
        vec!["restart", service.as_str()],
        vec!["pause", service.as_str()],
        vec!["continue", service.as_str()],
        vec!["rotate", service.as_str()],
        vec!["get", service.as_str(), "Description"],
        vec!["set", service.as_str(), "Description", "value"],
        vec!["reset", service.as_str(), "Description"],
        vec!["remove", service.as_str(), "confirm"],
    ] {
        let result = cli(&arguments);
        assert_eq!(
            result.status.code(),
            Some(3),
            "{} should return NSSM's missing-service status; stderr={}",
            arguments[0],
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            result.stdout.is_empty(),
            "{} unexpectedly wrote stdout",
            arguments[0]
        );
        if arguments[0] == "status" {
            assert!(
                result
                    .stderr
                    .starts_with(b"Can't open service!\r\r\nOpenService(): "),
                "missing-service diagnostic differs from NSSM's open-service message: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(
                result.stderr.ends_with(b"\r\r\n\r\r\n"),
                "missing-service diagnostic line endings differ from NSSM: {:?}",
                result.stderr
            );
        }
    }
    let status_code = cli(&["statuscode", &service]);
    assert_eq!(status_code.status.code(), Some(0));
    assert!(status_code.stdout.is_empty());
    assert!(
        status_code
            .stderr
            .starts_with(b"Can't open service!\r\r\nOpenService(): ")
    );
    assert!(status_code.stderr.ends_with(b"\r\r\n\r\r\n"));
}

#[test]
fn unknown_setting_is_reported_before_missing_service_lookup() {
    let service = unique_service_name();

    for args in [
        vec!["get", service.as_str(), "NssmRUnknownSetting"],
        vec!["set", service.as_str(), "NssmRUnknownSetting", "value"],
        vec!["reset", service.as_str(), "NssmRUnknownSetting"],
        vec!["unset", service.as_str(), "NssmRUnknownSetting"],
    ] {
        let output = cli(&args);
        assert_eq!(output.status.code(), Some(1), "args: {args:?}");
        assert!(output.stdout.is_empty(), "args: {args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.starts_with("Invalid parameter \"NssmRUnknownSetting\"."),
            "args: {args:?}, stderr: {stderr:?}"
        );
        assert!(
            !stderr.contains("OpenService"),
            "unknown setting was masked by missing service: {stderr:?}"
        );
    }
}

#[test]
fn processes_on_missing_service_returns_reference_command_status() {
    let service = unique_service_name();
    let result = cli(&["processes", &service]);
    assert_eq!(
        result.status.code(),
        Some(1),
        "processes should return NSSM's command failure status"
    );
    assert!(result.stdout.is_empty(), "missing service produced stdout");
    assert!(
        result
            .stderr
            .starts_with(b"Can't open service!\r\r\nEnumServicesStatus(): ")
    );
    assert!(result.stderr.ends_with(b"\r\r\n\r\r\n"));
}

#[test]
fn managed_settings_on_native_services_return_nssm_compatibility_error() {
    let service_type = cli(&["get", "Schedule", "Type"]);
    assert!(service_type.status.success());
    assert_eq!(
        String::from_utf8_lossy(&service_type.stdout).trim(),
        "SERVICE_WIN32_SHARE_PROCESS"
    );

    let result = cli(&["get", "Schedule", "AppRotateBytes"]);
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("Parameter \"AppRotateBytes\" is only valid for services managed by NSSM!")
    );
}

#[test]
fn native_service_queries_accept_display_names_and_return_the_canonical_name() {
    let key_name = cli(&["get", "Schedule", "Start"]);
    let display_name = cli(&["get", "Task Scheduler", "Start"]);
    assert!(key_name.status.success());
    assert!(
        display_name.status.success(),
        "display-name query failed: {}",
        String::from_utf8_lossy(&display_name.stderr)
    );
    assert_eq!(display_name.stdout, key_name.stdout);

    let canonical_name = cli(&["get", "Task Scheduler", "Name"]);
    assert!(canonical_name.status.success());
    assert_eq!(
        String::from_utf8_lossy(&canonical_name.stdout).trim(),
        "Schedule"
    );
}

#[test]
fn sends_ctrl_c_to_child_in_its_console_before_termination_fallback() {
    verify_service_shutdown_signal(&[], "6", "ctrl-c");
}

#[test]
fn sends_thread_quit_to_child_before_termination_fallback() {
    verify_service_shutdown_signal(&["thread"], "3", "thread-quit");
}

#[test]
fn sends_window_close_to_child_before_termination_fallback() {
    verify_service_shutdown_signal(&["window"], "5", "window-close");
}

#[test]
fn long_stop_wait_updates_status_and_logs_event_1078() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let fixture = std::env::var_os("NSSM_RS_TEST_FIXTURE")
        .or_else(|| option_env!("CARGO_BIN_EXE_console-signal-fixture").map(Into::into))
        .expect("Cargo builds the window shutdown fixture")
        .to_string_lossy()
        .into_owned();
    let marker = std::env::temp_dir().join(format!("{service}_delayed_window.txt"));
    let started_marker = std::env::temp_dir().join(format!("{service}_delayed_window.started"));
    let marker = marker.to_string_lossy().into_owned();
    let started_marker = started_marker.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![marker.clone(), started_marker.clone()]);

    let installed = cli(&["install", &service, &fixture, "window-delay"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    for (key, value) in [
        ("NSSM_SIGNAL_MARKER", marker.as_str()),
        ("NSSM_STARTED_MARKER", started_marker.as_str()),
    ] {
        assert!(
            cli(&["set", &service, "Environment", &format!("+{key}={value}")])
                .status
                .success()
        );
    }
    assert!(
        cli(&["set", &service, "AppStopMethodSkip", "5"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppStopMethodWindow", "24000"])
            .status
            .success()
    );
    assert!(cli(&["start", &service]).status.success());
    let start_deadline = Instant::now() + Duration::from_secs(10);
    while !std::path::Path::new(&started_marker).exists() && Instant::now() < start_deadline {
        thread::sleep(Duration::from_millis(25));
    }
    assert!(std::path::Path::new(&started_marker).exists());

    let binary = std::env::var_os("NSSM_RS_TEST_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_nssm-rs").into());
    let stopping = Command::new(binary)
        .args(["stop", &service])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("nssm-rs stop command starts");
    let progress_deadline = Instant::now() + Duration::from_secs(5);
    let mut checkpoint = 0;
    while Instant::now() < progress_deadline {
        if let Some(status) = service_status_snapshot(&service)
            && status.dwCurrentState == windows_sys::Win32::System::Services::SERVICE_STOP_PENDING
        {
            checkpoint = status.dwCheckPoint;
            if checkpoint > 0 {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    let stopped = stopping.wait_with_output().expect("stop command exits");
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(checkpoint > 0, "long stop wait advances the SCM checkpoint");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap_or_default(),
        "window-close",
        "the delayed window fixture received WM_CLOSE"
    );
    let stop_deadline = Instant::now() + Duration::from_secs(30);
    while !wait_for_service_state(&service, "SERVICE_STOPPED") && Instant::now() < stop_deadline {
        thread::sleep(Duration::from_millis(100));
    }
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "window-close");

    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; Id=1078}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ $p = $event.Properties; $values = ($p | ForEach-Object {{ $_.Value }}) -join '|'; Write-Output ('event 1078 provider=' + $event.ProviderName + ' values=' + $values); if ($event.ProviderName -eq 'nssm' -and $p.Count -eq 5 -and $p[0].Value -eq 'kill_process()' -and $p[1].Value -eq '{service}' -and $p[2].Value -eq 20000 -and $p[3].Value -eq 4000 -and $p[4].Value -eq 24000) {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "event 1078 reports the function, service, elapsed time, interval and timeout"
    );
}

#[test]
fn logs_child_process_launch_failures_to_the_application_event_log() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let missing_application = std::env::temp_dir()
        .join(format!("{service}_does_not_exist.exe"))
        .to_string_lossy()
        .into_owned();
    let installed = cli(&["install", &service, &missing_application]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );

    let started = cli(&["start", &service]);
    assert!(
        !started.status.success(),
        "start should fail when the configured application is missing"
    );
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));

    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1010}} -MaxEvents 40 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ $values = ($event.Properties | ForEach-Object {{ $_.Value }}) -join '|'; if ($values.Contains('{service}') -and $values.Contains('{missing_application}') -and $event.Properties.Count -ge 3 -and $event.Properties[2].Value -and $event.Message.Contains('Failed to start service')) {{ exit 0 }} }}; exit 1"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut event_found = false;
    while Instant::now() < deadline {
        event_found = hidden_powershell(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .is_ok_and(|status| status.success());
        if event_found {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    assert!(
        event_found,
        "Application log contains NSSM event 1010 with service and application values"
    );

    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1006}} -MaxEvents 40 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{missing_application}' -and $event.Properties[1].Value -eq '{service}' -and $event.Properties[2].Value -eq 3 -and $event.Message.Contains(\"Can't start\")) {{ exit 0 }} }}; exit 1"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut event_found = false;
    while Instant::now() < deadline {
        event_found = hidden_powershell(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .is_ok_and(|status| status.success());
        if event_found {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    assert!(
        event_found,
        "Application log contains NSSM event 1006 with the service-specific start failure code"
    );
    let status = Command::new("sc.exe")
        .args(["query", &service])
        .output()
        .expect("SCM status query runs");
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(
        status
            .lines()
            .any(|line| line.contains("WIN32_EXIT_CODE") && line.contains("1066")),
        "{status}"
    );
    assert!(
        status
            .lines()
            .any(|line| line.contains("SERVICE_EXIT_CODE") && line.contains("3")),
        "{status}"
    );
}

#[test]
fn logs_service_dispatcher_failures_to_the_application_event_log() {
    let output = cli(&["__service", &unique_service_name()]);
    assert!(
        !output.status.success(),
        "a service host started outside the SCM must fail"
    );

    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let script = "$events = Get-WinEvent -FilterHashtable @{LogName='Application'; ProviderName='nssm'; Id=1001} -MaxEvents 40 -ErrorAction SilentlyContinue; foreach ($event in $events) { if ($event.Properties.Count -eq 1 -and $event.Properties[0].Value -and $event.Message.Contains('StartServiceCtrlDispatcher() failed')) { exit 0 } }; exit 1";
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut event_found = false;
    while Instant::now() < deadline {
        event_found = hidden_powershell(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .status()
            .is_ok_and(|status| status.success());
        if event_found {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    assert!(
        event_found,
        "Application log contains NSSM event 1001 with the dispatcher error"
    );
}

#[test]
fn logs_successful_child_process_starts_to_the_application_event_log() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let application =
        std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()) + "\\System32\\PING.EXE";
    let installed = cli(&["install", &service, &application, "-t", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );

    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1008}} -MaxEvents 40 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 4 -and $event.Properties[0].Value -eq '{application}' -and $event.Properties[1].Value -eq '-t 127.0.0.1' -and $event.Properties[2].Value -eq '{service}' -and $event.Message.Contains('Started')) {{ exit 0 }} }}; exit 1"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut event_found = false;
    while Instant::now() < deadline {
        event_found = hidden_powershell(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .is_ok_and(|status| status.success());
        if event_found {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    assert!(
        event_found,
        "Application log contains NSSM event 1008 with executable, arguments and service"
    );
    assert!(cli(&["stop", &service]).status.success());

    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1011}} -MaxEvents 40 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 2 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq '{application}' -and $event.Message.Contains('Killing process')) {{ exit 0 }} }}; exit 1"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut event_found = false;
    while Instant::now() < deadline {
        event_found = hidden_powershell(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .is_ok_and(|status| status.success());
        if event_found {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    assert!(
        event_found,
        "Application log contains NSSM event 1011 with service and application values"
    );
}

#[test]
fn logs_application_exit_code_and_exit_action_to_the_event_log() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let installed = cli(&["install", &service, &command_prompt, "/c", "exit 7"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let policy = cli(&["set", &service, "AppExit", "Default", "Exit"]);
    assert!(
        policy.status.success(),
        "exit policy set failed: {}",
        String::from_utf8_lossy(&policy.stderr)
    );
    let _ = cli(&["start", &service]);

    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=@(1013,1016)}} -MaxEvents 60 -ErrorAction SilentlyContinue; $ended = $false; $policy = $false; foreach ($event in $events) {{ if ($event.Id -eq 1013 -and $event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{command_prompt}' -and $event.Properties[1].Value -eq '{service}' -and $event.Properties[2].Value -eq 7 -and $event.Message.Contains('exited with return code 7')) {{ $ended = $true }}; if ($event.Id -eq 1016 -and $event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq 7 -and $event.Properties[2].Value -eq 'Exit' -and $event.Message.Contains('Exiting')) {{ $policy = $true }} }}; if ($ended -and $policy) {{ exit 0 }}; exit 1"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events_found = false;
    while Instant::now() < deadline {
        events_found = hidden_powershell(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .is_ok_and(|status| status.success());
        if events_found {
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    assert!(
        events_found,
        "Application log contains events 1013 and 1016 with the child exit code and configured action"
    );
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
}

fn verify_service_shutdown_signal(arguments: &[&str], skip: &str, expected: &str) {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let marker = std::env::temp_dir().join(format!("{service}_signal.txt"));
    let marker = marker.to_string_lossy().into_owned();
    let started_marker = format!("{marker}.started");
    let _files = FileCleanup(vec![marker.clone(), started_marker.clone()]);
    let fixture = std::env::var_os("NSSM_RS_TEST_FIXTURE")
        .or_else(|| option_env!("CARGO_BIN_EXE_console-signal-fixture").map(Into::into))
        .expect("Cargo builds the console signal fixture");
    let fixture = fixture.to_string_lossy().into_owned();
    let mut install_args = vec!["install", &service, &fixture];
    install_args.extend_from_slice(arguments);
    let installed = cli(&install_args);
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    for (key, value) in [
        ("NSSM_SIGNAL_MARKER", marker.as_str()),
        ("NSSM_STARTED_MARKER", started_marker.as_str()),
    ] {
        let set_env = cli(&["set", &service, "Environment", &format!("+{key}={value}")]);
        assert!(
            set_env.status.success(),
            "{}",
            String::from_utf8_lossy(&set_env.stderr)
        );
    }
    let set_skip = cli(&["set", &service, "AppStopMethodSkip", skip]);
    assert!(
        set_skip.status.success(),
        "{}",
        String::from_utf8_lossy(&set_skip.stderr)
    );
    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_RUNNING"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !std::path::Path::new(&started_marker).exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        std::path::Path::new(&started_marker).exists(),
        "fixture starts with configured environment"
    );
    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&marker).expect("signal marker exists"),
        expected
    );
}

#[test]
fn stopping_service_terminates_descendant_processes() {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };

    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let script_path = std::env::temp_dir().join(format!("{service}.ps1"));
    let script_path = script_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![script_path.clone()]);
    std::fs::write(
        &script_path,
        "$ping = Join-Path $env:WINDIR 'System32\\PING.EXE'\nStart-Process -FilePath $ping -ArgumentList '-t 127.0.0.1' -WindowStyle Hidden\n& $ping -t 127.0.0.1\n",
    )
    .expect("process tree fixture is written");
    let installed = cli(&[
        "install",
        &service,
        &powershell,
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        &script_path,
    ]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let skip_graceful_methods = cli(&["set", &service, "AppStopMethodSkip", "7"]);
    assert!(
        skip_graceful_methods.status.success(),
        "stop method configuration failed: {}",
        String::from_utf8_lossy(&skip_graceful_methods.stderr)
    );
    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_RUNNING"));

    let mut process_handles = Vec::new();
    let mut descendant_pids = Vec::new();
    let mut last_tree = String::new();
    for _ in 0..100 {
        let tree = cli(&["processes", &service]);
        assert!(
            tree.status.success(),
            "process enumeration failed: {}",
            String::from_utf8_lossy(&tree.stderr)
        );
        last_tree = String::from_utf8_lossy(&tree.stdout).into_owned();
        let pids = last_tree
            .lines()
            .filter(|line| line.to_ascii_uppercase().contains("PING.EXE"))
            .filter_map(|line| line.split_whitespace().next()?.parse::<u32>().ok())
            .collect::<Vec<_>>();
        if pids.len() >= 2 {
            descendant_pids = pids.clone();
            process_handles = pids
                .into_iter()
                .filter_map(|pid| {
                    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
                    (!handle.is_null()).then_some(handle)
                })
                .collect();
            if process_handles.len() >= 2 {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(
        process_handles.len(),
        2,
        "the process tree did not contain both ping descendants: {last_tree}"
    );

    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    for handle in process_handles {
        assert_eq!(
            unsafe { WaitForSingleObject(handle, 5_000) },
            WAIT_OBJECT_0,
            "a descendant process remained alive after the service stopped"
        );
        unsafe { CloseHandle(handle) };
    }
    let pid_list = descendant_pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let event_script = format!(
        "$ids = @({pid_list}); $events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=@(1023,1027)}} -MaxEvents 300 -ErrorAction SilentlyContinue; $killing = @($events | Where-Object {{ $_.Id -eq 1023 -and $_.Properties.Count -ge 3 -and $_.Properties[0].Value -eq '{service}' -and $ids -contains [uint32]$_.Properties[1].Value -and $_.Properties[2].Value -eq '0' }}); $tree = @($events | Where-Object {{ $_.Id -eq 1027 -and $_.Properties.Count -ge 3 -and $ids -contains [uint32]$_.Properties[0].Value -and $_.Properties[2].Value -eq '{service}' }}); if ($killing.Count -ge 2 -and $tree.Count -ge 2) {{ exit 0 }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event_script),
        "Application log contains process-tree events 1023 and 1027 for both descendants"
    );
}

#[test]
fn skipped_termination_stops_service_and_logs_the_active_child() {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_TIMEOUT},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };

    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let _restore_stop_methods = RestoreStopMethods(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let ping = format!("{windows}\\System32\\PING.EXE");
    let installed = cli(&["install", &service, &ping, "-t", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppStopMethodSkip", "15"])
            .status
            .success()
    );
    assert!(cli(&["start", &service]).status.success());
    assert!(wait_for_service_state(&service, "SERVICE_RUNNING"));

    let deadline = Instant::now() + Duration::from_secs(10);
    let pid = loop {
        let tree = cli(&["processes", &service]);
        let pid = String::from_utf8_lossy(&tree.stdout)
            .lines()
            .find(|line| line.to_ascii_uppercase().contains("PING.EXE"))
            .and_then(|line| line.split_whitespace().next())
            .and_then(|value| value.parse::<u32>().ok());
        if let Some(pid) = pid {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "process tree did not expose the ping PID"
        );
        thread::sleep(Duration::from_millis(100));
    };
    let _process_cleanup = ProcessCleanup(pid);

    assert!(cli(&["stop", &service]).status.success());
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(
        !process.is_null(),
        "skipped child remains openable after service stop"
    );
    assert_eq!(
        unsafe { WaitForSingleObject(process, 0) },
        WAIT_TIMEOUT,
        "skipped child remains alive after service stop"
    );
    unsafe { CloseHandle(process) };

    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1053}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 4 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq {pid} -and $event.Properties[2].Value -eq 'NSSM' -and $event.Properties[3].Value -eq 'AppStopMethodSkip') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "event 1053 identifies the still-running process and skipped stop methods"
    );
}

#[test]
fn remove_prompts_and_honors_negative_and_positive_confirmation() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let ping = format!("{windows}\\System32\\PING.EXE");
    let installed = cli(&["install", &service, &ping, "-t", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );

    let declined = cli_with_input(&["remove", &service], "n\n");
    assert!(!declined.status.success());
    assert!(String::from_utf8_lossy(&declined.stderr).contains("removal cancelled"));
    let still_installed = cli(&["get", &service, "Name"]);
    assert!(still_installed.status.success());
    assert_eq!(
        String::from_utf8_lossy(&still_installed.stdout).trim(),
        service
    );

    let confirmed = cli_with_input(&["remove", &service], "yes\n");
    assert!(
        confirmed.status.success(),
        "confirmed removal failed: {}",
        String::from_utf8_lossy(&confirmed.stderr)
    );
    let mut disappeared = false;
    for _ in 0..100 {
        let status = cli(&["status", &service]);
        if !status.status.success() {
            disappeared = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(disappeared, "confirmed service removal did not take effect");
}

#[test]
fn missing_parameters_and_directory_log_warnings_and_use_reference_fallbacks() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let application = format!("{windows}\\System32\\whoami.exe");
    let fallback_directory = format!("{windows}\\System32");
    let installed = cli(&["install", &service, &application]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    for setting in ["AppParameters", "AppDirectory"] {
        let reset = cli(&["reset", &service, setting]);
        assert!(
            reset.status.success(),
            "reset {setting} failed: {}",
            String::from_utf8_lossy(&reset.stderr)
        );
    }
    assert!(
        cli(&["set", &service, "AppExit", "0", "Exit"])
            .status
            .success()
    );

    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "service start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=@(1029,1030)}} -MaxEvents 200 -ErrorAction SilentlyContinue; $flags = $false; $directory = $false; foreach ($event in $events) {{ $p = $event.Properties; if ($event.Id -eq 1029 -and $p.Count -ge 3 -and $p[0].Value -eq 'AppParameters' -and $p[1].Value -eq '{service}' -and $p[2].Value -ieq '{application}') {{ $flags = $true }}; if ($event.Id -eq 1030 -and $p.Count -ge 3 -and $p[0].Value -eq 'AppDirectory' -and $p[1].Value -eq '{service}' -and $p[2].Value -ieq '{fallback_directory}') {{ $directory = $true }} }}; if ($flags -and $directory) {{ exit 0 }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&script),
        "Application log contains 1029 and 1030 with the reference insertion values"
    );
    assert!(
        wait_for_service_state(&service, "SERVICE_STOPPED"),
        "whoami child should finish under the AppExit Exit policy"
    );
}

#[test]
fn missing_hook_executable_logs_event_and_does_not_abort_service_start() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let missing_hook = format!("{windows}\\System32\\nssm-rs-missing-hook.exe");
    let installed = cli(&["install", &service, &command_prompt, "/c", "exit 0"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppEvents", "Start/Pre", &missing_hook])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppExit", "0", "Ignore"])
            .status
            .success()
    );

    let _ = cli(&["start", &service]);
    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1080}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -ge 5 -and $event.Properties[0].Value -eq 'Start' -and $event.Properties[1].Value -eq 'Pre' -and $event.Properties[2].Value -eq '{service}' -and $event.Properties[3].Value -eq '{missing_hook}' -and $event.Properties[4].Value) {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&script),
        "Application log contains event 1080 with Start/Pre, service, and missing hook path"
    );
    assert!(
        wait_for_service_state(&service, "SERVICE_RUNNING"),
        "a failed Start/Pre hook launch does not abort the service start"
    );
    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
}

#[test]
fn unreadable_hook_configuration_logs_event_1081_and_does_not_abort_service_start() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let installed = cli(&["install", &service, &command_prompt, "/c", "exit 0"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppExit", "0", "Ignore"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppEvents", "Start/Pre", &command_prompt])
            .status
            .success()
    );

    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let registry_path =
        format!("SYSTEM\\CurrentControlSet\\Services\\{service}\\Parameters\\AppEvents\\Start");
    let deny_system_read = format!(
        "$path = '{registry_path}'; $key = [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($path, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree, [System.Security.AccessControl.RegistryRights]::ChangePermissions); if ($null -eq $key) {{ throw 'Unable to open hook key for ACL update' }}; try {{ $acl = $key.GetAccessControl(); $rule = New-Object System.Security.AccessControl.RegistryAccessRule('NT AUTHORITY\\SYSTEM', [System.Security.AccessControl.RegistryRights]::ReadKey, [System.Security.AccessControl.InheritanceFlags]::ContainerInherit, [System.Security.AccessControl.PropagationFlags]::None, [System.Security.AccessControl.AccessControlType]::Deny); $acl.AddAccessRule($rule); $key.SetAccessControl($acl) }} finally {{ $key.Close() }}"
    );
    let permission = hidden_powershell(&powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &deny_system_read,
        ])
        .output()
        .expect("PowerShell sets the temporary hook registry permission");
    assert!(
        permission.status.success(),
        "failed to deny SYSTEM read access to test hook key: {}",
        String::from_utf8_lossy(&permission.stderr)
    );

    let _ = cli(&["start", &service]);
    let event_script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1081}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -ge 3 -and $event.Properties[0].Value -eq 'Start' -and $event.Properties[1].Value -eq 'Pre' -and $event.Properties[2].Value -eq '{service}') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event_script),
        "Application log contains event 1081 for the unreadable Start/Pre hook"
    );
    assert!(
        wait_for_service_state(&service, "SERVICE_RUNNING"),
        "an unreadable Start/Pre hook does not abort service startup"
    );
}

#[test]
fn identical_stdout_and_stderr_paths_share_the_stdout_file_settings() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let output_path = std::env::temp_dir().join(format!("{service}_shared.log"));
    let output_path = output_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![output_path.clone()]);
    std::fs::write(&output_path, b"preserve-me\r\n").expect("seed shared output file");

    let installed = cli(&[
        "install",
        &service,
        &command_prompt,
        "/c",
        "echo child-output & echo child-error 1>&2",
    ]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    for (setting, value) in [
        ("AppStdout", output_path.clone()),
        ("AppStderr", output_path.to_ascii_uppercase()),
        ("AppStdoutCreationDisposition", "4".to_owned()),
        ("AppStderrCreationDisposition", "2".to_owned()),
        ("AppTimestampLog", "1".to_owned()),
    ] {
        let configured = cli(&["set", &service, setting, &value]);
        assert!(
            configured.status.success(),
            "setting {setting} failed: {}",
            String::from_utf8_lossy(&configured.stderr)
        );
    }
    assert!(
        cli(&["set", &service, "AppExit", "0", "Exit"])
            .status
            .success()
    );

    let _ = cli(&["start", &service]);
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
    let output = std::fs::read_to_string(&output_path).expect("shared output file exists");
    assert!(
        output.contains("preserve-me")
            && output.contains("child-output")
            && output.contains("child-error"),
        "NSSM applies stdout's OPEN_ALWAYS setting and routes both timestamped streams to one file: {output:?}"
    );
}

#[test]
fn redirects_child_stdout_and_stderr_to_configured_files() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let stdin_path = std::env::temp_dir().join(format!("{service} stdin.txt"));
    std::fs::write(&stdin_path, "stdin-marker\r\n").expect("stdin fixture is written");
    let stdout_path = std::env::temp_dir().join(format!("{service} stdout.log"));
    let stderr_path = std::env::temp_dir().join(format!("{service} stderr.log"));
    let stdin_path = stdin_path.to_string_lossy().into_owned();
    let stdout_path = stdout_path.to_string_lossy().into_owned();
    let stderr_path = stderr_path.to_string_lossy().into_owned();
    std::fs::write(&stdout_path, "existing-stdout\r\n").expect("stdout fixture is written");
    std::fs::write(&stderr_path, "existing-stderr\r\n").expect("stderr fixture is written");
    let _files = FileCleanup(vec![
        stdin_path.clone(),
        stdout_path.clone(),
        stderr_path.clone(),
    ]);
    let installed = cli(&[
        "install",
        &service,
        &command_prompt,
        "/c",
        "more & echo stdout-marker & echo stderr-marker 1>&2",
    ]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    for (setting, path) in [
        ("AppStdin", &stdin_path),
        ("AppStdout", &stdout_path),
        ("AppStderr", &stderr_path),
    ] {
        let configured = cli(&["set", &service, setting, path]);
        assert!(
            configured.status.success(),
            "{setting} set failed: {}",
            String::from_utf8_lossy(&configured.stderr)
        );
    }
    let stdout_disposition = cli(&["set", &service, "AppStdoutCreationDisposition", "2"]);
    assert!(
        stdout_disposition.status.success(),
        "stdout CREATE_ALWAYS configuration failed: {}",
        String::from_utf8_lossy(&stdout_disposition.stderr)
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );
    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let mut completed = false;
    for _ in 0..100 {
        let status = cli(&["status", &service]);
        let stdout_has_marker = std::fs::read_to_string(&stdout_path)
            .is_ok_and(|contents| contents.contains("stdout-marker"));
        let stderr_has_marker = std::fs::read_to_string(&stderr_path)
            .is_ok_and(|contents| contents.contains("stderr-marker"));
        if status.status.success()
            && String::from_utf8_lossy(&status.stdout).trim() == "SERVICE_STOPPED"
            && stdout_has_marker
            && stderr_has_marker
        {
            completed = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        completed,
        "service did not finish after writing both redirected streams"
    );
    let stdout = std::fs::read_to_string(&stdout_path).expect("stdout file was created");
    let stderr = std::fs::read_to_string(&stderr_path).expect("stderr file was created");
    assert!(
        stdout.contains("stdout-marker"),
        "unexpected stdout content: {stdout:?}"
    );
    assert!(
        stdout.contains("stdin-marker"),
        "stdin was not delivered to the child: {stdout:?}"
    );
    assert!(
        !stdout.contains("existing-stdout"),
        "stdout CREATE_ALWAYS retained existing data: {stdout:?}"
    );
    assert!(
        !stdout.contains("stderr-marker"),
        "stderr leaked into stdout: {stdout:?}"
    );
    assert!(
        stderr.contains("stderr-marker"),
        "unexpected stderr content: {stderr:?}"
    );
    assert!(
        stderr.contains("existing-stderr"),
        "stderr append mode truncated existing data: {stderr:?}"
    );
    assert!(
        !stderr.contains("stdout-marker"),
        "stdout leaked into stderr: {stderr:?}"
    );
}

#[test]
fn passes_replacement_and_expanded_extra_environment_to_child() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let stdout_path = std::env::temp_dir().join(format!("{service}_environment.txt"));
    let stdout_path = stdout_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![stdout_path.clone()]);
    let installed = cli(&[
        "install",
        &service,
        &command_prompt,
        "/c",
        "echo %NSSMR_BASE%:%NSSMR_DERIVED%",
    ]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppEnvironment", "NSSMR_BASE=base"])
            .status
            .success()
    );
    let base = cli(&["get", &service, "AppEnvironment", "NSSMR_BASE"]);
    assert!(base.status.success());
    assert_eq!(String::from_utf8_lossy(&base.stdout).trim(), "base");
    assert!(
        cli(&[
            "set",
            &service,
            "AppEnvironmentExtra",
            "NSSMR_DERIVED=%NSSMR_BASE%-extra",
        ])
        .status
        .success()
    );
    let derived = cli(&["get", &service, "AppEnvironmentExtra", "NSSMR_DERIVED"]);
    assert!(derived.status.success());
    assert_eq!(
        String::from_utf8_lossy(&derived.stdout).trim(),
        "%NSSMR_BASE%-extra"
    );
    assert!(
        cli(&["set", &service, "AppStdout", &stdout_path])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );
    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let mut completed = false;
    for _ in 0..100 {
        let status = cli(&["status", &service]);
        completed = status.status.success()
            && String::from_utf8_lossy(&status.stdout).trim() == "SERVICE_STOPPED"
            && std::fs::read_to_string(&stdout_path)
                .is_ok_and(|contents| contents.trim() == "base:base-extra");
        if completed {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        completed,
        "child did not receive the composed environment: {:?}",
        std::fs::read_to_string(&stdout_path)
    );
    for (setting, key) in [
        ("AppEnvironment", "NSSMR_BASE"),
        ("AppEnvironmentExtra", "NSSMR_DERIVED"),
    ] {
        let reset = cli(&["reset", &service, setting]);
        assert!(
            reset.status.success(),
            "reset {setting} failed: {}",
            String::from_utf8_lossy(&reset.stderr)
        );
        let queried = cli(&["get", &service, setting, key]);
        assert!(queried.status.success());
        assert_eq!(
            String::from_utf8_lossy(&queried.stdout).trim(),
            "",
            "reset {setting} retained {key}"
        );
    }
}

#[test]
fn launches_unicode_application_path_and_preserves_windows_arguments() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let working_directory = std::env::temp_dir().join(format!("{service}_雪"));
    std::fs::create_dir_all(&working_directory).expect("unicode working directory is created");
    let application = working_directory.join("服务 fixture.exe");
    let fixture = std::env::var_os("NSSM_RS_TEST_FIXTURE")
        .or_else(|| option_env!("CARGO_BIN_EXE_console-signal-fixture").map(Into::into))
        .expect("console signal fixture is built");
    std::fs::copy(fixture, &application).expect("fixture is copied to unicode path");
    let capture_path = working_directory.join("参数输出.txt");
    let captured_arguments = [
        "plain".to_owned(),
        "two words".to_owned(),
        "雪与火".to_owned(),
        "embedded\"quote".to_owned(),
        "trailing slash\\".to_owned(),
        String::new(),
    ];
    let raw_parameters = std::iter::once("capture-args".to_owned())
        .chain(std::iter::once(nssm_rs::quote_windows_arg(
            &capture_path.to_string_lossy(),
        )))
        .chain(
            captured_arguments
                .iter()
                .map(|argument| nssm_rs::quote_windows_arg(argument)),
        )
        .collect::<Vec<_>>()
        .join(" ");
    let application = application.to_string_lossy().into_owned();
    assert!(cli(&["install", &service, &application]).status.success());
    assert!(
        cli(&["set", &service, "AppParameters", &raw_parameters])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );

    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "service start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(
        wait_for_service_state(&service, "SERVICE_STOPPED"),
        "service did not stop after its argument-capture child exited"
    );
    assert_eq!(
        std::fs::read_to_string(&capture_path).expect("child captures received arguments"),
        captured_arguments.join("\n")
    );
    std::fs::remove_dir_all(working_directory).expect("unicode fixture directory is removed");
}

#[test]
fn applies_configured_priority_and_cpu_affinity_to_child_process() {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};

    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let mut process_affinity = 0usize;
    let mut system_affinity = 0usize;
    assert_ne!(
        unsafe {
            GetProcessAffinityMask(
                GetCurrentProcess(),
                &mut process_affinity,
                &mut system_affinity,
            )
        },
        0,
        "test runner affinity is queryable"
    );
    let cpu = system_affinity.trailing_zeros();
    let affinity = 1usize << cpu;
    let capture_path = std::env::temp_dir().join(format!("{service}_process_info.txt"));
    let capture_path = capture_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![capture_path.clone()]);
    let fixture = std::env::var_os("NSSM_RS_TEST_FIXTURE")
        .or_else(|| option_env!("CARGO_BIN_EXE_console-signal-fixture").map(Into::into))
        .expect("console signal fixture is built");
    let fixture = fixture.to_string_lossy().into_owned();
    assert!(
        cli(&["install", &service, &fixture]).status.success(),
        "fixture service installs"
    );
    let parameters = format!(
        "capture-process-info {}",
        nssm_rs::quote_windows_arg(&capture_path)
    );
    assert!(
        cli(&["set", &service, "AppParameters", &parameters])
            .status
            .success()
    );
    assert!(
        cli(&[
            "set",
            &service,
            "AppPriority",
            "BELOW_NORMAL_PRIORITY_CLASS"
        ])
        .status
        .success()
    );
    assert!(
        cli(&["set", &service, "AppAffinity", &cpu.to_string()])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );
    assert!(cli(&["start", &service]).status.success());
    assert!(
        wait_for_service_state(&service, "SERVICE_STOPPED"),
        "service did not stop after process information was captured"
    );

    let captured = std::fs::read_to_string(&capture_path)
        .expect("child process reports effective priority and affinity");
    let mut fields = captured.lines();
    assert_eq!(fields.next(), Some("16384"));
    let actual_affinity = usize::from_str_radix(fields.next().unwrap(), 16).unwrap();
    let child_system_affinity = usize::from_str_radix(fields.next().unwrap(), 16).unwrap();
    assert_eq!(actual_affinity, affinity);
    assert_eq!(child_system_affinity & affinity, affinity);
}

#[test]
fn invalid_runtime_registry_values_log_events_and_fall_back_to_defaults() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let capture_path = std::env::temp_dir().join(format!("{service}_priority.txt"));
    let capture_path = capture_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![capture_path.clone()]);
    let fixture = std::env::var_os("NSSM_RS_TEST_FIXTURE")
        .or_else(|| option_env!("CARGO_BIN_EXE_console-signal-fixture").map(Into::into))
        .expect("process information fixture is built");
    let fixture = fixture.to_string_lossy().into_owned();
    assert!(
        cli(&["install", &service, &fixture]).status.success(),
        "fixture service installs"
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success(),
        "fixture exits after the test child completes"
    );
    let parameters = format!(
        "capture-process-info {}",
        nssm_rs::quote_windows_arg(&capture_path)
    );
    assert!(
        cli(&["set", &service, "AppParameters", &parameters])
            .status
            .success(),
        "fixture captures the effective process priority"
    );

    let registry_key = format!("HKLM:\\SYSTEM\\CurrentControlSet\\Services\\{service}\\Parameters");
    let write_invalid_priority = format!(
        "New-ItemProperty -LiteralPath '{registry_key}' -Name AppPriority -PropertyType DWord -Value 65536 -Force; New-ItemProperty -LiteralPath '{registry_key}' -Name AppAffinity -PropertyType String -Value 'not-a-cpu-range' -Force; New-ItemProperty -LiteralPath '{registry_key}' -Name AppThrottle -PropertyType String -Value 'invalid-throttle' -Force; New-ItemProperty -LiteralPath '{registry_key}' -Name AppRestartDelay -PropertyType String -Value 'invalid-delay' -Force; New-ItemProperty -LiteralPath '{registry_key}' -Name AppNoConsole -PropertyType String -Value 'invalid-no-console' -Force; New-ItemProperty -LiteralPath '{registry_key}' -Name AppKillProcessTree -PropertyType String -Value 'invalid-kill-tree' -Force"
    );
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let written = hidden_powershell(&powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &write_invalid_priority,
        ])
        .status()
        .expect("PowerShell writes the isolated service setting");
    assert!(written.success(), "invalid priority value is written");

    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "invalid priority falls back instead of preventing startup: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
    let captured = std::fs::read_to_string(&capture_path)
        .expect("child process reports its effective priority");
    assert_eq!(
        captured.lines().next(),
        Some("32"),
        "invalid AppPriority starts the child at NORMAL_PRIORITY_CLASS"
    );

    let escaped_service = service.replace('\'', "''");
    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1066}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 2 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq 'AppPriority') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "invalid AppPriority logs event 1066 with service name and setting"
    );
    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1067}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 2 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq 'not-a-cpu-range') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "invalid AppAffinity logs event 1067 with service name and requested mask"
    );
    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1036}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq 'AppThrottle' -and $event.Properties[2].Value -eq '1500') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "invalid AppThrottle falls back to 1500 ms and logs event 1036"
    );
    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1071}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq 'AppRestartDelay' -and $event.Properties[2].Value -eq '0') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "invalid AppRestartDelay falls back to 0 ms and logs event 1071"
    );
    for setting in ["AppNoConsole", "AppKillProcessTree"] {
        let event = format!(
            "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1018}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 2 -and $event.Properties[0].Value -eq '{setting}') {{ exit 0 }} }}; exit 1"
        );
        assert!(
            wait_for_windows_powershell_condition(&event),
            "invalid {setting} type logs event 1018 and uses the setting default"
        );
    }
}

#[test]
fn malformed_environment_entries_report_event_1038_before_service_start_fails() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    assert!(
        cli(&["install", &service, &command_prompt, "/c", "exit 0"])
            .status
            .success(),
        "temporary service installs"
    );

    let registry_key = format!("HKLM:\\SYSTEM\\CurrentControlSet\\Services\\{service}\\Parameters");
    let write_invalid_environment = format!(
        "New-ItemProperty -LiteralPath '{registry_key}' -Name AppEnvironmentExtra -PropertyType MultiString -Value @('invalid-without-assignment') -Force"
    );
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let written = hidden_powershell(&powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &write_invalid_environment,
        ])
        .status()
        .expect("PowerShell writes an invalid REG_MULTI_SZ value");
    assert!(written.success(), "invalid environment value is written");

    let started = cli(&["start", &service]);
    assert!(
        !started.status.success(),
        "malformed environment prevents child startup"
    );
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));

    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1038}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ $p = $event.Properties; if ($p.Count -eq 3 -and $p[0].Value -eq '{service}' -and $p[1].Value -eq '{command_prompt}' -and $p[2].Value -eq 'AppEnvironmentExtra' -and $event.Message.Contains('ERROR_INVALID_PARAMETER')) {{ exit 0 }} }}; exit 1"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if hidden_powershell(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &event])
            .status()
            .is_ok_and(|status| status.success())
        {
            return;
        }
        thread::sleep(Duration::from_millis(250));
    }
    panic!("Application log contains event 1038 for AppEnvironmentExtra");
}

#[test]
fn unavailable_affinity_bits_log_effective_mask_and_constrain_the_child() {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};

    let mut process_mask = 0usize;
    let mut system_mask = 0usize;
    assert_ne!(
        unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process_mask, &mut system_mask) },
        0,
        "test runner affinity is queryable"
    );
    let unavailable_cpu = (0..usize::BITS)
        .find(|cpu| system_mask & (1usize << cpu) == 0)
        .expect("Windows exposes at most 64 processors in the tested process group");
    let requested_mask = system_mask | (1usize << unavailable_cpu);
    let requested = nssm_rs::format_cpu_affinity_mask(requested_mask as u64).unwrap();
    let system = nssm_rs::format_cpu_affinity_mask(system_mask as u64).unwrap();

    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let capture_path = std::env::temp_dir().join(format!("{service}_affinity.txt"));
    let capture_path = capture_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![capture_path.clone()]);
    let fixture = std::env::var_os("NSSM_RS_TEST_FIXTURE")
        .or_else(|| option_env!("CARGO_BIN_EXE_console-signal-fixture").map(Into::into))
        .expect("process information fixture is built");
    let fixture = fixture.to_string_lossy().into_owned();
    assert!(cli(&["install", &service, &fixture]).status.success());
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );
    let parameters = format!(
        "capture-process-info {}",
        nssm_rs::quote_windows_arg(&capture_path)
    );
    assert!(
        cli(&["set", &service, "AppParameters", &parameters])
            .status
            .success()
    );

    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let registry_key = format!("HKLM:\\SYSTEM\\CurrentControlSet\\Services\\{service}\\Parameters");
    let set_affinity = format!(
        "New-ItemProperty -LiteralPath '{registry_key}' -Name AppAffinity -PropertyType String -Value '{requested}' -Force"
    );
    assert!(
        hidden_powershell(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &set_affinity])
            .status()
            .is_ok_and(|status| status.success()),
        "test writes a valid mask containing one unavailable CPU"
    );

    assert!(cli(&["start", &service]).status.success());
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
    let captured = std::fs::read_to_string(&capture_path)
        .expect("child reports its effective processor affinity");
    let mut values = captured.lines();
    assert_eq!(values.next(), Some("32"));
    assert_eq!(
        values.next(),
        Some(format!("{:x}", system_mask).as_str()),
        "unavailable affinity bits are removed before launching the child"
    );

    let escaped_service = service.replace('\'', "''");
    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1068}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ $p = $event.Properties; if ($p.Count -eq 4 -and $p[0].Value -eq '{escaped_service}' -and $p[1].Value -eq '{requested}' -and $p[2].Value -eq '{system}' -and $p[3].Value -eq '{system}') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "event 1068 records requested, supported, and effective affinity masks"
    );
}

#[test]
fn invalid_stop_method_skip_type_logs_event_and_keeps_all_stop_methods() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    assert!(
        cli(&["install", &service, &command_prompt, "/c", "exit 0"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );

    let registry_key = format!("HKLM:\\SYSTEM\\CurrentControlSet\\Services\\{service}\\Parameters");
    let write_invalid_type = format!(
        "New-ItemProperty -LiteralPath '{registry_key}' -Name AppStopMethodSkip -PropertyType String -Value 'not-a-dword-value' -Force"
    );
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    assert!(
        hidden_powershell(&powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &write_invalid_type
            ])
            .status()
            .is_ok_and(|status| status.success()),
        "test writes the wrong registry value type"
    );

    assert!(cli(&["start", &service]).status.success());
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
    let escaped_service = service.replace('\'', "''");
    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1052}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ $p = $event.Properties; if ($p.Count -eq 3 -and $p[0].Value -eq '{escaped_service}' -and $p[1].Value -eq 'AppStopMethodSkip' -and $p[2].Value -eq 'NSSM') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "event 1052 identifies the service, value, and reference product"
    );
}

#[test]
fn short_non_dword_stop_skip_value_uses_the_successfully_read_data() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    assert!(
        cli(&["install", &service, &command_prompt, "/c", "exit 0"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );

    let registry_key = format!("HKLM:\\SYSTEM\\CurrentControlSet\\Services\\{service}\\Parameters");
    let write_short_string = format!(
        "New-ItemProperty -LiteralPath '{registry_key}' -Name AppStopMethodSkip -PropertyType String -Value '' -Force"
    );
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    assert!(
        hidden_powershell(&powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &write_short_string
            ])
            .status()
            .is_ok_and(|status| status.success())
    );

    let _ = cli(&["start", &service]);
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
    let escaped_service = service.replace('\'', "''");
    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1052}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{escaped_service}') {{ exit 1 }} }}; exit 0"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "successful short registry read does not trigger event 1052"
    );
}

#[test]
fn invalid_stop_grace_period_type_uses_default_and_logs_event() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let marker = std::env::temp_dir().join(format!("{service}_signal.txt"));
    let marker = marker.to_string_lossy().into_owned();
    let started_marker = format!("{marker}.started");
    let _files = FileCleanup(vec![marker.clone(), started_marker.clone()]);
    let fixture = std::env::var_os("NSSM_RS_TEST_FIXTURE")
        .or_else(|| option_env!("CARGO_BIN_EXE_console-signal-fixture").map(Into::into))
        .expect("Cargo builds the console signal fixture")
        .to_string_lossy()
        .into_owned();
    assert!(cli(&["install", &service, &fixture]).status.success());
    for (key, value) in [
        ("NSSM_SIGNAL_MARKER", marker.as_str()),
        ("NSSM_STARTED_MARKER", started_marker.as_str()),
    ] {
        assert!(
            cli(&["set", &service, "Environment", &format!("+{key}={value}")])
                .status
                .success()
        );
    }

    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let registry_key = format!("HKLM:\\SYSTEM\\CurrentControlSet\\Services\\{service}\\Parameters");
    let write_invalid_type = format!(
        "New-ItemProperty -LiteralPath '{registry_key}' -Name AppStopMethodConsole -PropertyType String -Value 'invalid-delay' -Force"
    );
    assert!(
        hidden_powershell(&powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &write_invalid_type
            ])
            .status()
            .is_ok_and(|status| status.success()),
        "test writes a wrong-type stop grace period"
    );

    assert!(cli(&["start", &service]).status.success());
    assert!(wait_for_service_state(&service, "SERVICE_RUNNING"));
    let start_deadline = Instant::now() + Duration::from_secs(5);
    while !std::path::Path::new(&started_marker).exists() && Instant::now() < start_deadline {
        thread::sleep(Duration::from_millis(25));
    }
    assert!(std::path::Path::new(&started_marker).exists());
    assert!(cli(&["stop", &service]).status.success());
    assert!(std::path::Path::new(&marker).exists());

    let escaped_service = service.replace('\'', "''");
    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1056}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq 'AppStopMethodConsole' -and $event.Properties[2].Value -eq '1500') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "invalid grace period uses the 1500 ms default and logs event 1056"
    );
}

#[test]
fn timestamp_log_prefixes_each_child_output_line_in_utc() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let output_path = std::env::temp_dir().join(format!("{service}_timestamps.log"));
    let output_path = output_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![output_path.clone()]);
    let installed = cli(&[
        "install",
        &service,
        &command_prompt,
        "/c",
        "echo first-marker & timeout /t 1 /nobreak >nul & echo second-marker",
    ]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppStdout", &output_path])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppTimestampLog", "1"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );

    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(
        wait_for_service_state(&service, "SERVICE_STOPPED"),
        "one-shot child did not finish"
    );
    let output = std::fs::read_to_string(&output_path).expect("timestamp log exists");
    let lines = output.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "unexpected timestamped log: {output:?}");
    for (line, marker) in lines.into_iter().zip(["first-marker", "second-marker"]) {
        assert!(
            line.contains(marker),
            "timestamped line lost child output: {line:?}"
        );
        let (timestamp, _) = line.split_once(": ").expect("timestamp separator exists");
        assert_eq!(timestamp.len(), 23, "unexpected UTC timestamp: {timestamp}");
        assert_eq!(&timestamp[4..5], "-");
        assert_eq!(&timestamp[7..8], "-");
        assert_eq!(&timestamp[10..11], " ");
        assert_eq!(&timestamp[13..14], ":");
        assert_eq!(&timestamp[16..17], ":");
        assert_eq!(&timestamp[19..20], ".");
    }
}

#[test]
fn timestamp_log_preserves_utf16_streams_and_writes_a_bom() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let output_path = std::env::temp_dir().join(format!("{service}_utf16.log"));
    let output_path = output_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![output_path.clone()]);
    let script = "JABiAHkAdABlAHMAIAA9ACAAWwBUAGUAeAB0AC4ARQBuAGMAbwBkAGkAbgBnAF0AOgA6AFUAbgBpAGMAbwBkAGUALgBHAGUAdABCAHkAdABlAHMAKAAnAHcAaQBkAGUALQBtAGEAcgBrAGUAcgAnACAAKwAgAFsAYwBoAGEAcgBdADEAMwAgACsAIABbAGMAaABhAHIAXQAxADAAKQA7ACAAWwBDAG8AbgBzAG8AbABlAF0AOgA6AE8AcABlAG4AUwB0AGEAbgBkAGEAcgBkAE8AdQB0AHAAdQB0ACgAKQAuAFcAcgBpAHQAZQAoACQAYgB5AHQAZQBzACwAIAAwACwAIAAkAGIAeQB0AGUAcwAuAEwAZQBuAGcAdABoACkA";
    let installed = cli(&[
        "install",
        &service,
        &powershell,
        "-NoProfile",
        "-NonInteractive",
        "-EncodedCommand",
        script,
    ]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppStdout", &output_path])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppTimestampLog", "1"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );
    assert!(cli(&["start", &service]).status.success());
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));

    let output = std::fs::read(&output_path).expect("UTF-16 timestamp log exists");
    assert!(
        output.starts_with(&[0xff, 0xfe]),
        "missing UTF-16LE BOM: {output:?}"
    );
    let (pairs, remainder) = output[2..].as_chunks::<2>();
    assert!(
        remainder.is_empty(),
        "UTF-16LE output has an incomplete code unit"
    );
    let code_units = pairs
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect::<Vec<_>>();
    let output = String::from_utf16(&code_units).expect("timestamped output remains UTF-16");
    let (timestamp, _) = output
        .split_once(": wide-marker")
        .expect("timestamped child text is present");
    assert_eq!(timestamp.len(), 23, "unexpected UTC timestamp: {timestamp}");
}

#[test]
fn runs_configured_start_and_exit_hooks() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let events = ["Start_Pre", "Start_Post", "Exit_Post"]
        .map(|event| std::env::temp_dir().join(format!("{service}_{event}.txt")));
    let event_paths = events
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let hook_working_directory = std::env::temp_dir().join(&service);
    std::fs::create_dir(&hook_working_directory).expect("hook working directory is created");
    let cwd_marker = std::env::temp_dir().join(format!("{service}_hook_cwd.txt"));
    let cwd_marker = cwd_marker.to_string_lossy().into_owned();
    let abi_marker = std::env::temp_dir().join(format!("{service}_hook_abi.txt"));
    let abi_marker = abi_marker.to_string_lossy().into_owned();
    let exit_abi_marker = std::env::temp_dir().join(format!("{service}_exit_hook_abi.txt"));
    let exit_abi_marker = exit_abi_marker.to_string_lossy().into_owned();
    let _files = FileCleanup(
        [
            event_paths.clone(),
            vec![
                cwd_marker.clone(),
                abi_marker.clone(),
                exit_abi_marker.clone(),
            ],
        ]
        .concat(),
    );
    let _directory = DirectoryCleanup(hook_working_directory.to_string_lossy().into_owned());
    let installed = cli(&["install", &service, &command_prompt, "/c", "exit 0"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "DisplayName", "Hook ABITest Service"])
            .status
            .success()
    );
    let hook_working_directory = hook_working_directory.to_string_lossy().into_owned();
    assert!(
        cli(&["set", &service, "AppDirectory", &hook_working_directory])
            .status
            .success()
    );
    for ((event_action, marker), path) in [
        ("Start/Pre", "start-pre %NSSM_EVENT% %NSSM_ACTION% %NSSM_TRIGGER% %NSSM_DEADLINE% %NSSM_CONFIGURATION% %NSSM_START_REQUESTED_COUNT% %NSSM_START_COUNT% %NSSM_EXIT_COUNT% %NSSM_THROTTLE_COUNT% %NSSM_RUNTIME% %NSSM_LAST_CONTROL% %NSSM_SERVICE_DISPLAYNAME% %NSSM_BUILD_DATE%"),
        ("Start/Post", ""),
        ("Exit/Post", "exit-post %NSSM_EXIT_COUNT% %NSSM_START_COUNT% %NSSM_APPLICATION_RUNTIME%"),
    ]
    .into_iter()
    .zip(&event_paths)
    {
        let hook = if event_action == "Start/Post" {
            format!("cmd.exe /c set NSSM_COMMAND_LINE>{path}")
        } else if event_action == "Start/Pre" {
            format!(
                "cmd.exe /c echo {marker}>{path} & cd > {cwd_marker} & set NSSM_ > {abi_marker}"
            )
        } else if event_action == "Exit/Post" {
            format!("cmd.exe /c echo {marker}>{path} & set NSSM_ > {exit_abi_marker}")
        } else {
            format!("cmd.exe /c echo {marker}>{path}")
        };
        let configured = cli(&["set", &service, "AppEvents", event_action, &hook]);
        assert!(
            configured.status.success(),
            "hook set failed: {}",
            String::from_utf8_lossy(&configured.stderr)
        );
    }
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );
    let _ = cli(&["start", &service]);
    let mut hooks_ran = false;
    for _ in 0..100 {
        hooks_ran = event_paths
            .iter()
            .all(|path| std::path::Path::new(path).is_file())
            && std::path::Path::new(&exit_abi_marker).is_file()
            && std::fs::read_to_string(&event_paths[1])
                .is_ok_and(|contents| contents.starts_with("NSSM_COMMAND_LINE=\""))
            && std::fs::read_to_string(&event_paths[2])
                .is_ok_and(|contents| contents.trim().starts_with("exit-post 1 "));
        if hooks_ran {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        hooks_ran,
        "lifecycle hook output did not complete: start_pre={:?}, start_post={:?}, exit_post={:?}",
        std::fs::read_to_string(&event_paths[0]),
        std::fs::read_to_string(&event_paths[1]),
        std::fs::read_to_string(&event_paths[2]),
    );
    let start_hook = std::fs::read_to_string(&event_paths[0]).unwrap();
    let mut fields = start_hook.split_whitespace();
    assert_eq!(fields.next(), Some("start-pre"));
    assert_eq!(fields.next(), Some("Start"));
    assert_eq!(fields.next(), Some("Pre"));
    assert_eq!(fields.next(), Some("START"));
    assert_eq!(fields.next(), Some("20000"));
    assert_eq!(fields.next(), Some("64-bit"));
    if cfg!(debug_assertions) {
        assert_eq!(fields.next(), Some("debug"));
    }
    assert_eq!(fields.next(), Some("1"));
    assert_eq!(fields.next(), Some("0"));
    assert_eq!(fields.next(), Some("0"));
    assert_eq!(fields.next(), Some("0"));
    assert!(fields.next().unwrap().parse::<u64>().unwrap() < 60_000);
    assert_eq!(fields.next(), Some("START"));
    assert_eq!(fields.next(), Some("Hook"));
    assert_eq!(fields.next(), Some("ABITest"));
    assert_eq!(fields.next(), Some("Service"));
    assert_eq!(fields.next(), Some(env!("NSSM_BUILD_DATE")));
    assert_eq!(
        std::fs::read_to_string(&cwd_marker)
            .expect("hook writes its current directory")
            .trim(),
        hook_working_directory
    );
    let hook_environment = read_hook_environment(&abi_marker);
    assert_eq!(
        hook_environment
            .get("NSSM_HOOK_VERSION")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        hook_environment.get("NSSM_VERSION").map(String::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        hook_environment.get("NSSM_BUILD_DATE").map(String::as_str),
        Some(env!("NSSM_BUILD_DATE"))
    );
    assert_eq!(
        hook_environment
            .get("NSSM_SERVICE_NAME")
            .map(String::as_str),
        Some(service.as_str())
    );
    assert_eq!(
        hook_environment
            .get("NSSM_SERVICE_DISPLAYNAME")
            .map(String::as_str),
        Some("Hook ABITest Service")
    );
    assert_eq!(
        hook_environment
            .get("NSSM_APPLICATION_PID")
            .map(String::as_str),
        Some("")
    );
    assert_eq!(
        hook_environment.get("NSSM_EXITCODE").map(String::as_str),
        Some("")
    );
    assert_eq!(
        hook_environment
            .get("NSSM_APPLICATION_RUNTIME")
            .map(String::as_str),
        Some("")
    );
    assert!(hook_environment["NSSM_PID"].parse::<u32>().unwrap() > 0);
    assert!(std::path::Path::new(&hook_environment["NSSM_EXE"]).is_file());
    assert_hook_abi_present(&hook_environment, "Start/Pre");
    let architecture = if cfg!(target_pointer_width = "64") {
        "64-bit"
    } else {
        "32-bit"
    };
    let configuration = if cfg!(debug_assertions) {
        format!("{architecture} debug")
    } else {
        architecture.to_owned()
    };
    assert_eq!(hook_environment["NSSM_EVENT"], "Start");
    assert_eq!(hook_environment["NSSM_ACTION"], "Pre");
    assert_eq!(hook_environment["NSSM_TRIGGER"], "START");
    assert_eq!(hook_environment["NSSM_LAST_CONTROL"], "START");
    assert_eq!(hook_environment["NSSM_CONFIGURATION"], configuration);
    assert_eq!(hook_environment["NSSM_DEADLINE"], "20000");
    assert_eq!(hook_environment["NSSM_START_REQUESTED_COUNT"], "1");
    assert_eq!(hook_environment["NSSM_START_COUNT"], "0");
    assert_eq!(hook_environment["NSSM_EXIT_COUNT"], "0");
    assert_eq!(hook_environment["NSSM_THROTTLE_COUNT"], "0");
    assert!(hook_environment["NSSM_RUNTIME"].parse::<u64>().is_ok());
    assert!(hook_environment["NSSM_COMMAND_LINE"].starts_with('"'));

    let exit_environment = read_hook_environment(&exit_abi_marker);
    assert_hook_abi_present(&exit_environment, "Exit/Post");
    assert_eq!(exit_environment["NSSM_EVENT"], "Exit");
    assert_eq!(exit_environment["NSSM_ACTION"], "Post");
    assert_eq!(exit_environment["NSSM_TRIGGER"], "");
    assert_eq!(exit_environment["NSSM_APPLICATION_PID"], "");
    assert_eq!(exit_environment["NSSM_EXITCODE"], "0");
    assert_eq!(exit_environment["NSSM_EXIT_COUNT"], "1");
    assert!(
        exit_environment["NSSM_APPLICATION_RUNTIME"]
            .parse::<u64>()
            .is_ok()
    );
    let start_post = std::fs::read_to_string(&event_paths[1]).unwrap();
    assert!(start_post.starts_with("NSSM_COMMAND_LINE=\""));
    assert!(start_post.contains("cmd.exe\""));
    let exit_hook = std::fs::read_to_string(&event_paths[2]).unwrap();
    assert!(exit_hook.trim().starts_with("exit-post 1 1 "));
    assert!(
        exit_hook
            .split_whitespace()
            .nth(3)
            .unwrap()
            .parse::<u64>()
            .is_ok()
    );
}

#[test]
fn start_pre_hook_exit_99_aborts_application_launch() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let hook_path = std::env::temp_dir().join(format!("{service}_hook.txt"));
    let app_path = std::env::temp_dir().join(format!("{service}_app.txt"));
    let hook_path = hook_path.to_string_lossy().into_owned();
    let app_path = app_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![hook_path.clone(), app_path.clone()]);
    let application = format!("echo app-started>\"{app_path}\"");
    let installed = cli(&["install", &service, &command_prompt, "/c", &application]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );
    let hook = format!("cmd.exe /c echo hook-fired>\"{hook_path}\" & exit 99");
    let configured = cli(&["set", &service, "AppEvents", "Start/Pre", &hook]);
    assert!(
        configured.status.success(),
        "hook set failed: {}",
        String::from_utf8_lossy(&configured.stderr)
    );

    let start_time = Instant::now();
    let started = cli(&["start", &service]);
    assert!(
        !started.status.success(),
        "start reported success after the Start/Pre hook aborted startup"
    );
    assert!(
        start_time.elapsed() < Duration::from_secs(5),
        "start waited too long after SCM had already stopped the service: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let mut aborted = false;
    for _ in 0..100 {
        let status = cli(&["status", &service]);
        let stopped = status.status.success()
            && String::from_utf8_lossy(&status.stdout).trim() == "SERVICE_STOPPED";
        let hook_ran = std::fs::read_to_string(&hook_path)
            .is_ok_and(|contents| contents.trim() == "hook-fired");
        if stopped && hook_ran {
            aborted = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        aborted,
        "Start/Pre hook did not run and abort startup: {:?}",
        std::fs::read_to_string(&hook_path)
    );
    let escaped_service = service.replace('\'', "''");
    let event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1079}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 4 -and $event.Properties[0].Value -eq 'START' -and $event.Properties[1].Value -eq 'PRE' -and $event.Properties[2].Value -eq '{escaped_service}' -and $event.Properties[3].Value -eq '99') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event),
        "event 1079 records the aborted Start/Pre hook and exit code"
    );
    thread::sleep(Duration::from_millis(250));
    assert!(
        !std::path::Path::new(&app_path).exists(),
        "application was launched after Start/Pre hook returned 99"
    );
}

#[test]
fn rotates_and_stops_through_the_configured_hooks_in_order() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let ping = format!("{windows}\\System32\\PING.EXE");
    let marker_path = std::env::temp_dir().join(format!("{service}_hook_order.txt"));
    let marker_path = marker_path.to_string_lossy().into_owned();
    let rotate_pre_abi = std::env::temp_dir().join(format!("{service}_rotate_pre_abi.txt"));
    let rotate_pre_abi = rotate_pre_abi.to_string_lossy().into_owned();
    let rotate_post_abi = std::env::temp_dir().join(format!("{service}_rotate_post_abi.txt"));
    let rotate_post_abi = rotate_post_abi.to_string_lossy().into_owned();
    let stop_pre_abi = std::env::temp_dir().join(format!("{service}_stop_pre_abi.txt"));
    let stop_pre_abi = stop_pre_abi.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![
        marker_path.clone(),
        rotate_pre_abi.clone(),
        rotate_post_abi.clone(),
        stop_pre_abi.clone(),
    ]);
    let installed = cli(&["install", &service, &ping, "-t", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let online_rotation = cli(&["set", &service, "AppRotateOnline", "1"]);
    assert!(
        online_rotation.status.success(),
        "online rotation set failed: {}",
        String::from_utf8_lossy(&online_rotation.stderr)
    );
    for (event, action, environment_path) in [
        ("Rotate/Pre", "rotate-pre", &rotate_pre_abi),
        ("Rotate/Post", "rotate-post", &rotate_post_abi),
        ("Stop/Pre", "stop-pre", &stop_pre_abi),
    ] {
        let hook = format!(
            "cmd.exe /c echo {action}>>\"{marker_path}\"&set NSSM_ >\"{environment_path}\""
        );
        let configured = cli(&["set", &service, "AppEvents", event, &hook]);
        assert!(
            configured.status.success(),
            "{event} hook set failed: {}",
            String::from_utf8_lossy(&configured.stderr)
        );
    }
    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_RUNNING"));

    let rotated = cli(&["rotate", &service]);
    assert!(
        rotated.status.success(),
        "rotate failed: {}",
        String::from_utf8_lossy(&rotated.stderr)
    );
    let mut rotation_hooks_ran = false;
    for _ in 0..100 {
        rotation_hooks_ran = std::fs::read_to_string(&marker_path).is_ok_and(|contents| {
            contents.lines().collect::<Vec<_>>() == ["rotate-pre", "rotate-post"]
        });
        if rotation_hooks_ran {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        rotation_hooks_ran,
        "rotation hook order was wrong: {:?}",
        std::fs::read_to_string(&marker_path)
    );

    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let hooks = std::fs::read_to_string(&marker_path).expect("hook marker exists");
    assert_eq!(
        hooks.lines().collect::<Vec<_>>(),
        ["rotate-pre", "rotate-post", "stop-pre"]
    );
    for path in [&rotate_pre_abi, &rotate_post_abi, &stop_pre_abi] {
        assert!(
            std::path::Path::new(path).is_file(),
            "missing hook ABI: {path}"
        );
    }
    let rotate_pre = read_hook_environment(&rotate_pre_abi);
    let rotate_post = read_hook_environment(&rotate_post_abi);
    let stop_pre = read_hook_environment(&stop_pre_abi);
    assert_hook_abi_present(&rotate_pre, "Rotate/Pre");
    assert_hook_abi_present(&rotate_post, "Rotate/Post");
    assert_hook_abi_present(&stop_pre, "Stop/Pre");
    for environment in [&rotate_pre, &rotate_post] {
        assert_eq!(environment["NSSM_EVENT"], "Rotate");
        assert_eq!(environment["NSSM_TRIGGER"], "ROTATE");
        assert_eq!(environment["NSSM_LAST_CONTROL"], "ROTATE");
        assert_eq!(environment["NSSM_EXITCODE"], "");
        assert!(environment["NSSM_APPLICATION_PID"].parse::<u32>().is_ok());
        assert!(
            environment["NSSM_APPLICATION_RUNTIME"]
                .parse::<u64>()
                .is_ok()
        );
    }
    assert_eq!(rotate_pre["NSSM_ACTION"], "Pre");
    assert_eq!(rotate_post["NSSM_ACTION"], "Post");
    assert_eq!(rotate_pre["NSSM_DEADLINE"], "60000");
    assert_eq!(rotate_post["NSSM_DEADLINE"], "60000");
    assert_eq!(stop_pre["NSSM_EVENT"], "Stop");
    assert_eq!(stop_pre["NSSM_ACTION"], "Pre");
    assert_eq!(stop_pre["NSSM_TRIGGER"], "STOP");
    assert_eq!(stop_pre["NSSM_LAST_CONTROL"], "STOP");
    assert_eq!(stop_pre["NSSM_DEADLINE"], "20000");
    assert_eq!(stop_pre["NSSM_EXITCODE"], "");
    assert!(stop_pre["NSSM_APPLICATION_PID"].parse::<u32>().is_ok());
    assert!(stop_pre["NSSM_APPLICATION_RUNTIME"].parse::<u64>().is_ok());
}

#[test]
fn redirects_hook_stdout_and_stderr_when_enabled() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let ping = format!("{windows}\\System32\\PING.EXE");
    let stdout_path = std::env::temp_dir().join(format!("{service}_hook_stdout.log"));
    let stderr_path = std::env::temp_dir().join(format!("{service}_hook_stderr.log"));
    let stdout_path = stdout_path.to_string_lossy().into_owned();
    let stderr_path = stderr_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![stdout_path.clone(), stderr_path.clone()]);
    let installed = cli(&["install", &service, &ping, "-t", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    for (setting, path) in [("AppStdout", &stdout_path), ("AppStderr", &stderr_path)] {
        let configured = cli(&["set", &service, setting, path]);
        assert!(
            configured.status.success(),
            "{setting} set failed: {}",
            String::from_utf8_lossy(&configured.stderr)
        );
    }
    assert!(
        cli(&["set", &service, "AppRedirectHook", "1"])
            .status
            .success()
    );
    let hook = "cmd.exe /c echo hook-stdout & echo hook-stderr 1>&2";
    let configured = cli(&["set", &service, "AppEvents", "Start/Pre", hook]);
    assert!(
        configured.status.success(),
        "hook set failed: {}",
        String::from_utf8_lossy(&configured.stderr)
    );

    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_RUNNING"));
    let mut redirected = false;
    for _ in 0..100 {
        let stdout = std::fs::read(&stdout_path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        let stderr = std::fs::read(&stderr_path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        if stdout.contains("hook-stdout") && stderr.contains("hook-stderr") {
            redirected = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        redirected,
        "hook output was not redirected to configured files: stdout={:?}, stderr={:?}",
        std::fs::read(&stdout_path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
        std::fs::read(&stderr_path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    );
    let stdout = String::from_utf8_lossy(&std::fs::read(&stdout_path).expect("stdout log exists"))
        .into_owned();
    let stderr = String::from_utf8_lossy(&std::fs::read(&stderr_path).expect("stderr log exists"))
        .into_owned();
    assert!(stdout.contains("hook-stdout"), "stdout log: {stdout:?}");
    assert!(!stdout.contains("hook-stderr"), "stdout log: {stdout:?}");
    assert!(stderr.contains("hook-stderr"), "stderr log: {stderr:?}");
    assert!(!stderr.contains("hook-stdout"), "stderr log: {stderr:?}");

    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
}

#[test]
fn restarts_child_for_configured_exit_code_until_service_is_stopped() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let marker_path = std::env::temp_dir().join(format!("{service}_restart.txt"));
    let marker_path = marker_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![marker_path.clone()]);
    let installed = cli(&["install", &service, &command_prompt, "/c", "exit 23"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppExit", "23", "Restart"])
            .status
            .success()
    );
    let hook = format!("cmd.exe /c echo exit>>\"{marker_path}\"");
    assert!(
        cli(&["set", &service, "AppEvents", "Exit/Post", &hook])
            .status
            .success()
    );

    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let mut repeated = false;
    for _ in 0..100 {
        repeated = std::fs::read_to_string(&marker_path)
            .is_ok_and(|contents| contents.lines().count() >= 3);
        if repeated {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        repeated,
        "exit policy did not restart the application three times"
    );
    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1014}} -MaxEvents 60 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 4 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq 23 -and $event.Properties[2].Value -eq 'Restart' -and $event.Properties[3].Value -eq '{command_prompt}' -and $event.Message.Contains('Attempting to restart')) {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&script),
        "Application log contains event 1014 with the exit code, Restart action and executable"
    );
    let throttle_script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1034}} -MaxEvents 60 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq 1500 -and $event.Properties[2].Value -eq 2000) {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&throttle_script),
        "Application log contains event 1034 with the throttle threshold and retry delay"
    );
    let status = cli(&["status", &service]);
    let current_state = String::from_utf8_lossy(&status.stdout).trim().to_owned();
    assert!(
        status.status.success()
            && matches!(
                current_state.as_str(),
                "SERVICE_START_PENDING" | "SERVICE_RUNNING" | "SERVICE_PAUSED"
            ),
        "service left the active restart lifecycle with state {current_state}"
    );

    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
}

#[test]
fn failed_restart_launch_stops_service_then_retries_after_delay() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let source = format!("{windows}\\System32\\cmd.exe");
    let application = std::env::temp_dir().join(format!("{service}.exe"));
    let unavailable = std::env::temp_dir().join(format!("{service}.missing.exe"));
    let application = application.to_string_lossy().into_owned();
    let unavailable = unavailable.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![application.clone(), unavailable.clone()]);
    std::fs::copy(source, &application).expect("copy cmd.exe to disposable application path");

    let installed = cli(&["install", &service, &application, "/c", "exit 23"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppExit", "23", "Restart"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppRestartDelay", "5000"])
            .status
            .success()
    );
    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut restart_delay_observed = false;
    while Instant::now() < deadline {
        let status = cli(&["status", &service]);
        if status.status.success()
            && String::from_utf8_lossy(&status.stdout).trim() == "SERVICE_PAUSED"
        {
            restart_delay_observed = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        restart_delay_observed,
        "first app exit entered restart delay"
    );
    std::fs::rename(&application, &unavailable)
        .expect("make application unavailable during the configured restart delay");

    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=@(1006,1007,1010,1012)}} -MaxEvents 300 -ErrorAction SilentlyContinue; $launch = $false; $alreadyStopped = $false; $start = $false; $retry = $false; foreach ($event in $events) {{ if ($event.Id -eq 1010 -and $event.Properties.Count -ge 2 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq '{application}') {{ $launch = $true }}; if ($event.Id -eq 1012 -and $event.Properties.Count -ge 2 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq '{application}') {{ $alreadyStopped = $true }}; if ($event.Id -eq 1006 -and $event.Properties.Count -ge 3 -and $event.Properties[0].Value -eq '{application}' -and $event.Properties[1].Value -eq '{service}' -and $event.Properties[2].Value -eq 3) {{ $start = $true }}; if ($event.Id -eq 1007 -and $event.Properties.Count -ge 2 -and $event.Properties[0].Value -eq '{application}' -and $event.Properties[1].Value -eq '{service}') {{ $retry = $true }} }}; if ($launch -and $alreadyStopped -and $start -and $retry) {{ exit 0 }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&script),
        "Application log records CreateProcess failure 1010, already-stopped event 1012, service start failure 1006, and retry warning 1007"
    );
    let retry_delay_started = Instant::now();
    assert!(
        wait_for_service_state(&service, "SERVICE_STOPPED"),
        "SCM observes SERVICE_STOPPED while NSSM waits before retrying"
    );
    std::fs::rename(&unavailable, &application)
        .expect("restore the application before NSSM's 30-second retry");
    let retry_deadline = Instant::now() + Duration::from_secs(40);
    let mut restarted = false;
    while Instant::now() < retry_deadline {
        let status = cli(&["status", &service]);
        if status.status.success()
            && String::from_utf8_lossy(&status.stdout).trim() == "SERVICE_RUNNING"
        {
            restarted = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let final_status = cli(&["status", &service]);
    assert!(
        restarted,
        "NSSM retries the application after 30 seconds; status={} stderr={}",
        String::from_utf8_lossy(&final_status.stdout).trim(),
        String::from_utf8_lossy(&final_status.stderr).trim()
    );
    assert!(
        retry_delay_started.elapsed() >= Duration::from_secs(29),
        "restart retry must wait approximately 30 seconds"
    );
    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop after successful retry failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
}

#[test]
fn stdio_file_creation_failure_logs_reference_events_and_stops_startup() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let missing_directory = std::env::temp_dir().join(format!("{service}_missing"));
    let stdout_path = missing_directory
        .join("stdout.log")
        .to_string_lossy()
        .into_owned();
    let _files = FileCleanup(vec![stdout_path.clone()]);

    let installed = cli(&["install", &service, &command_prompt, "/c", "exit 0"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let configured = cli(&["set", &service, "AppStdout", &stdout_path]);
    assert!(
        configured.status.success(),
        "stdout configuration failed: {}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let _ = cli(&["start", &service]);
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));

    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=@(1006,1012,1049,1051)}} -MaxEvents 300 -ErrorAction SilentlyContinue; $create = $false; $handles = $false; $stopped = $false; $start = $false; foreach ($event in $events) {{ if ($event.Id -eq 1049 -and $event.Properties.Count -ge 2 -and $event.Properties[0].Value -eq '{stdout_path}') {{ $create = $true }}; if ($event.Id -eq 1051 -and $event.Properties.Count -ge 1 -and $event.Properties[0].Value -eq '{service}') {{ $handles = $true }}; if ($event.Id -eq 1012 -and $event.Properties.Count -ge 2 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq '{command_prompt}') {{ $stopped = $true }}; if ($event.Id -eq 1006 -and $event.Properties.Count -ge 3 -and $event.Properties[0].Value -eq '{command_prompt}' -and $event.Properties[1].Value -eq '{service}' -and $event.Properties[2].Value -eq 4) {{ $start = $true }} }}; if ($create -and $handles -and $stopped -and $start) {{ exit 0 }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&script),
        "Application log records CreateFile failure 1049, output setup failure 1051, already-stopped event 1012, and service start failure 1006 with exit code 4"
    );
    assert!(
        !std::path::Path::new(&stdout_path).exists(),
        "the service must not create the configured log beneath the missing directory"
    );
}

#[test]
fn invalid_stdio_numeric_setting_logs_query_and_startup_failure_events() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let stdout_path = std::env::temp_dir()
        .join(format!("{service}_stdout.log"))
        .to_string_lossy()
        .into_owned();
    let _files = FileCleanup(vec![stdout_path.clone()]);
    assert!(
        cli(&["install", &service, &command_prompt, "/c", "exit 0"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppStdout", &stdout_path])
            .status
            .success()
    );

    let registry_key = format!("HKLM:\\SYSTEM\\CurrentControlSet\\Services\\{service}\\Parameters");
    let write_invalid_value = format!(
        "New-ItemProperty -LiteralPath '{registry_key}' -Name AppStdoutCopyAndTruncate -PropertyType String -Value 'invalid-copy-flag' -Force"
    );
    let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    assert!(
        hidden_powershell(&powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &write_invalid_value
            ])
            .status()
            .is_ok_and(|status| status.success())
    );

    let _ = cli(&["start", &service]);
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=@(1006,1012,1018,1051)}} -MaxEvents 300 -ErrorAction SilentlyContinue; $query = $false; $handles = $false; $stopped = $false; $start = $false; foreach ($event in $events) {{ if ($event.Id -eq 1018 -and $event.Properties.Count -eq 2 -and $event.Properties[0].Value -eq 'AppStdoutCopyAndTruncate') {{ $query = $true }}; if ($event.Id -eq 1051 -and $event.Properties.Count -ge 1 -and $event.Properties[0].Value -eq '{service}') {{ $handles = $true }}; if ($event.Id -eq 1012 -and $event.Properties.Count -ge 2 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq '{command_prompt}') {{ $stopped = $true }}; if ($event.Id -eq 1006 -and $event.Properties.Count -ge 3 -and $event.Properties[0].Value -eq '{command_prompt}' -and $event.Properties[1].Value -eq '{service}' -and $event.Properties[2].Value -eq 4) {{ $start = $true }} }}; if ($query -and $handles -and $stopped -and $start) {{ exit 0 }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&script),
        "invalid stdio flag logs query error 1018, output setup failure 1051, already-stopped event 1012, and start failure 1006"
    );
}

#[test]
fn ignore_and_exit_actions_have_distinct_service_lifecycles() {
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    for (action, expected_state) in [("Ignore", "SERVICE_RUNNING"), ("Exit", "SERVICE_STOPPED")] {
        let service = unique_service_name();
        let _cleanup = Cleanup(service.clone());
        let exit_path = std::env::temp_dir().join(format!("{service}_exit_action.txt"));
        let exit_path = exit_path.to_string_lossy().into_owned();
        let _files = FileCleanup(vec![exit_path.clone()]);
        let application = "ping -n 2 127.0.0.1 >NUL & exit 23";
        let installed = cli(&["install", &service, &command_prompt, "/c", application]);
        assert!(
            installed.status.success(),
            "install failed: {}",
            String::from_utf8_lossy(&installed.stderr)
        );
        assert!(
            cli(&["set", &service, "AppExit", "Default", "Exit"])
                .status
                .success()
        );
        let configured_action = cli(&["set", &service, "AppExit", "23", action]);
        assert!(
            configured_action.status.success(),
            "{action} exit rule set failed: {}",
            String::from_utf8_lossy(&configured_action.stderr)
        );
        let hook = format!("cmd.exe /c echo exit-%NSSM_EXITCODE%>>\"{exit_path}\"");
        let configured_hook = cli(&["set", &service, "AppEvents", "Exit/Post", &hook]);
        assert!(
            configured_hook.status.success(),
            "Exit/Post hook set failed: {}",
            String::from_utf8_lossy(&configured_hook.stderr)
        );

        let _ = cli(&["start", &service]);
        let mut observed = false;
        for _ in 0..100 {
            let status = cli(&["status", &service]);
            let current_state = String::from_utf8_lossy(&status.stdout).trim().to_owned();
            let exit_hook = std::fs::read_to_string(&exit_path).unwrap_or_default();
            if current_state == expected_state && exit_hook.trim() == "exit-23" {
                observed = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            observed,
            "{action} did not produce {expected_state} with exit code 23; status={:?}, hook={:?}",
            cli(&["status", &service]).stdout,
            std::fs::read_to_string(&exit_path)
        );
        let event_script = if action == "Ignore" {
            format!(
                "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1015}} -MaxEvents 60 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 4 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq 23 -and $event.Properties[2].Value -eq 'Ignore' -and $event.Properties[3].Value -eq '{command_prompt}' -and $event.Message.Contains('No action will be taken')) {{ exit 0 }} }}; exit 1"
            )
        } else {
            format!(
                "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1016}} -MaxEvents 60 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq 23 -and $event.Properties[2].Value -eq 'Exit' -and $event.Message.Contains('Exiting')) {{ exit 0 }} }}; exit 1"
            )
        };
        assert!(
            wait_for_windows_powershell_condition(&event_script),
            "Application log contains event {} for the {action} exit policy",
            if action == "Ignore" { 1015 } else { 1016 }
        );
        if action == "Ignore" {
            thread::sleep(Duration::from_millis(500));
            assert_eq!(
                std::fs::read_to_string(&exit_path)
                    .expect("exit hook marker exists")
                    .lines()
                    .count(),
                1,
                "Ignore restarted the application"
            );
            let stopped = cli(&["stop", &service]);
            assert!(
                stopped.status.success(),
                "stop failed: {}",
                String::from_utf8_lossy(&stopped.stderr)
            );
        }
    }
}

#[test]
fn forced_service_stop_reports_zero_exit_code_to_exit_hook() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let exit_path = std::env::temp_dir().join(format!("{service}_stop_exit_code.txt"));
    let exit_path = exit_path.to_string_lossy().into_owned();
    let _files = FileCleanup(vec![exit_path.clone()]);

    let installed = cli(&[
        "install",
        &service,
        &command_prompt,
        "/c",
        "ping -t 127.0.0.1 >NUL",
    ]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppStopMethodSkip", "7"])
            .status
            .success()
    );
    let hook = format!("cmd.exe /c echo exit-%NSSM_EXITCODE%>>\"{exit_path}\"");
    let configured_hook = cli(&["set", &service, "AppEvents", "Exit/Post", &hook]);
    assert!(
        configured_hook.status.success(),
        "Exit/Post hook set failed: {}",
        String::from_utf8_lossy(&configured_hook.stderr)
    );

    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_RUNNING"));

    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
    let mut observed = false;
    for _ in 0..100 {
        if std::fs::read_to_string(&exit_path).is_ok_and(|contents| contents.trim() == "exit-0") {
            observed = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        observed,
        "forced stop did not report exit code 0 to Exit/Post: {:?}",
        std::fs::read_to_string(&exit_path)
    );
}

#[test]
fn suicide_exit_action_terminates_the_service_host_as_a_failure() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let installed = cli(&[
        "install",
        &service,
        &command_prompt,
        "/c",
        "ping -n 2 127.0.0.1 >NUL & exit 23",
    ]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppExit", "23", "Suicide"])
            .status
            .success()
    );
    let _ = cli(&["start", &service]);
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));

    let escaped_service = service.replace('\'', "''");
    let script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='System'; ProviderName='Service Control Manager'; Id=7034}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -gt 0 -and $event.Properties[0].Value -eq '{escaped_service}') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&script),
        "SCM did not record event 7034 for the service host process terminated by Suicide"
    );
    let event_script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1020}} -MaxEvents 60 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq 23 -and $event.Properties[2].Value -eq 'Suicide') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event_script),
        "Application log contains event 1020 with the Suicide policy and exit code"
    );
}

#[test]
fn default_suicide_on_zero_exit_stops_gracefully_and_logs_the_nssm_diagnostic() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let installed = cli(&["install", &service, &command_prompt, "/c", "exit 0"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let configured = cli(&["set", &service, "AppExit", "Default", "Suicide"]);
    assert!(
        configured.status.success(),
        "default Suicide action set failed: {}",
        String::from_utf8_lossy(&configured.stderr)
    );

    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(
        wait_for_service_state(&service, "SERVICE_STOPPED"),
        "default Suicide for exit code zero did not stop the service cleanly"
    );

    let graceful_suicide = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1021}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 6 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq '{command_prompt}' -and $event.Properties[2].Value -eq 'Suicide' -and $event.Properties[3].Value -eq 'Suicide' -and $event.Properties[4].Value -eq 'Suicide' -and $event.Properties[5].Value -eq 'Exit' -and $event.Message.Contains('exited with exit code 0')) {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&graceful_suicide),
        "Application log contains event 1021 with NSSM's default Suicide fallback values"
    );
    let exit_action = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1016}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{service}' -and $event.Properties[1].Value -eq 0 -and $event.Properties[2].Value -eq 'Suicide') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&exit_action),
        "Application log preserves Suicide as the exit action before graceful fallback"
    );
}

#[test]
fn continue_interrupts_restart_throttle_while_pause_leaves_it_waiting() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let ping = format!("{windows}\\System32\\PING.EXE");
    let installed = cli(&["install", &service, &ping, "-n", "2", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppRestartDelay", "30000"])
            .status
            .success()
    );
    assert!(cli(&["start", &service]).status.success());
    assert!(
        wait_for_service_state(&service, "SERVICE_PAUSED"),
        "service did not enter the NSSM restart-throttle paused state"
    );
    let escaped_service = service.replace('\'', "''");
    let restart_delay_script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1072}} -MaxEvents 60 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 2 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq 30000) {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&restart_delay_script),
        "Application log contains event 1072 with the configured restart delay"
    );
    let paused_again = cli(&["pause", &service]);
    assert!(
        paused_again.status.success(),
        "pause control failed: {}",
        String::from_utf8_lossy(&paused_again.stderr)
    );
    assert!(
        wait_for_service_state(&service, "SERVICE_PAUSED"),
        "Pause unexpectedly resumed the throttled service"
    );
    let pause_control_event = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1041}} -MaxEvents 500 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 2 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq 'PAUSE') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&pause_control_event),
        "Application log contains event 1041 for the unsupported PAUSE control"
    );
    let continued = cli(&["continue", &service]);
    assert!(
        continued.status.success(),
        "continue failed: {}",
        String::from_utf8_lossy(&continued.stderr)
    );
    assert!(
        wait_for_service_state(&service, "SERVICE_RUNNING"),
        "service did not return to running after Continue"
    );
    let event_script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1035}} -MaxEvents 60 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 1 -and $event.Properties[0].Value -eq '{escaped_service}') {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event_script),
        "Application log contains event 1035 for resetting the restart throttle on Continue"
    );
    assert!(
        wait_for_service_state(&service, "SERVICE_PAUSED"),
        "service did not re-enter the restart-throttle wait after Continue"
    );
    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop during restart throttle failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_STOPPED"));
    let already_stopped_script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1012}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 2 -and $event.Properties[0].Value -eq '{escaped_service}' -and $event.Properties[1].Value -eq '{ping}' -and $event.Message.Contains('No action is required')) {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&already_stopped_script),
        "Application log contains event 1012 when stopping during the child-free throttle wait"
    );
}

#[test]
fn startup_rotation_preserves_existing_log_and_creates_a_fresh_active_file() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let log_path = std::env::temp_dir().join(format!("{service}_startup.log"));
    let log_path = log_path.to_string_lossy().into_owned();
    let prefix = format!("{service}_startup-");
    let _log_cleanup = RotationCleanup {
        active: log_path.clone(),
        prefix: prefix.clone(),
    };
    std::fs::write(&log_path, "previous-run-marker\r\n").expect("existing log is created");
    let installed = cli(&[
        "install",
        &service,
        &command_prompt,
        "/c",
        "echo current-run-marker",
    ]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppStdout", &log_path])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppRotateFiles", "1"])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );
    let _ = cli(&["start", &service]);
    let mut current_run_written = false;
    for _ in 0..100 {
        current_run_written = std::fs::read_to_string(&log_path)
            .is_ok_and(|contents| contents.contains("current-run-marker"));
        if current_run_written {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        current_run_written,
        "new service run did not write to the active log"
    );
    let backup = std::env::temp_dir()
        .read_dir()
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
        })
        .expect("startup rotation created a timestamped backup");
    let escaped_service = service.replace('\'', "''");
    let escaped_prefix = prefix.replace('\'', "''");
    let rotation_event_script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1077}} -MaxEvents 500 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ if ($event.Properties.Count -eq 3 -and $event.Properties[0].Value -eq '{escaped_service}' -and ([string]$event.Properties[1].Value).EndsWith('{escaped_service}_startup.log') -and ([string]$event.Properties[2].Value).Contains('{escaped_prefix}')) {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&rotation_event_script),
        "Application log contains event 1077 with the service's active and rotated log paths"
    );
    let contents = std::fs::read_to_string(backup).expect("rotated startup log can be read");
    assert!(contents.contains("previous-run-marker"));
    assert!(!contents.contains("current-run-marker"));
}

#[test]
fn failed_startup_log_rotation_reports_reference_event_and_still_launches_child() {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let command_prompt = format!("{windows}\\System32\\cmd.exe");
    let log_path = std::env::temp_dir().join(format!("{service}_rotation_failure.log"));
    let prefix = format!("{service}_rotation_failure-");
    let log_path_string = log_path.to_string_lossy().into_owned();
    let _log_cleanup = RotationCleanup {
        active: log_path_string.clone(),
        prefix,
    };
    std::fs::write(&log_path, "rotation collision marker\r\n")
        .expect("existing output log is created");
    let modified = std::fs::metadata(&log_path)
        .and_then(|metadata| metadata.modified())
        .expect("active log modification time is available");
    let rotated_path =
        nssm_rs::rotated_log_path(&log_path, modified).expect("rotated filename is generated");
    std::fs::create_dir(&rotated_path).expect("rotation target collision is created");

    let installed = cli(&[
        "install",
        &service,
        &command_prompt,
        "/c",
        "echo child-started",
    ]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    for (setting, value) in [
        ("AppStdout", log_path_string.as_str()),
        ("AppRotateFiles", "1"),
    ] {
        let configured = cli(&["set", &service, setting, value]);
        assert!(
            configured.status.success(),
            "setting {setting} failed: {}",
            String::from_utf8_lossy(&configured.stderr)
        );
    }
    assert!(
        cli(&["set", &service, "AppExit", "Default", "Exit"])
            .status
            .success()
    );
    let _ = cli(&["start", &service]);
    assert!(
        wait_for_service_state(&service, "SERVICE_STOPPED"),
        "child did not finish after startup rotation failed"
    );
    assert!(
        std::fs::read_to_string(&log_path).is_ok_and(|contents| contents.contains("child-started")),
        "log rotation failure prevented application output"
    );

    let service_query = service.replace('\'', "''");
    let active_query = log_path_string.replace('\'', "''");
    let rotated_query = rotated_path.to_string_lossy().replace('\'', "''");
    let event_script = format!(
        "$events = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1063}} -MaxEvents 100 -ErrorAction SilentlyContinue; foreach ($event in $events) {{ $p = $event.Properties; if ($p.Count -eq 5 -and $p[0].Value -eq '{service_query}' -and $p[1].Value -eq '{active_query}' -and $p[2].Value -eq 'MoveFile()' -and $p[3].Value -eq '{rotated_query}' -and $p[4].Value) {{ exit 0 }} }}; exit 1"
    );
    assert!(
        wait_for_windows_powershell_condition(&event_script),
        "Application log contains event 1063 with the failed rotation operation and paths"
    );
}

#[test]
fn rotate_command_moves_the_active_log_and_reopens_it() {
    verify_live_rotation(false);
}

#[test]
fn rotate_command_copy_truncates_when_configured() {
    verify_live_rotation(true);
}

fn verify_live_rotation(copy_and_truncate: bool) {
    let service = unique_service_name();
    let _cleanup = Cleanup(service.clone());
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let ping = format!("{windows}\\System32\\PING.EXE");
    let log_path = std::env::temp_dir().join(format!("{service}_rotation.log"));
    let log_path = log_path.to_string_lossy().into_owned();
    let prefix = format!("{service}_rotation-");
    let _log_cleanup = RotationCleanup {
        active: log_path.clone(),
        prefix,
    };
    let installed = cli(&["install", &service, &ping, "-t", "127.0.0.1"]);
    assert!(
        installed.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert!(
        cli(&["set", &service, "AppStdout", &log_path])
            .status
            .success()
    );
    assert!(
        cli(&["set", &service, "AppRotateOnline", "1"])
            .status
            .success()
    );
    if copy_and_truncate {
        assert!(
            cli(&["set", &service, "AppStdoutCopyAndTruncate", "1"])
                .status
                .success()
        );
    }
    let configured_copy_mode = cli(&["get", &service, "AppStdoutCopyAndTruncate"]);
    assert!(configured_copy_mode.status.success());
    assert_eq!(
        String::from_utf8_lossy(&configured_copy_mode.stdout).trim(),
        if copy_and_truncate { "1" } else { "0" }
    );
    assert!(cli(&["start", &service]).status.success());
    let mut has_output = false;
    for _ in 0..100 {
        has_output = std::fs::metadata(&log_path).is_ok_and(|metadata| metadata.len() > 0);
        if has_output {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(has_output, "child did not write its initial output");
    let original_file_identity = file_identity(&log_path);
    thread::sleep(Duration::from_millis(1100));
    let rotated = cli(&["rotate", &service]);
    assert!(
        rotated.status.success(),
        "rotate failed: {}",
        String::from_utf8_lossy(&rotated.stderr)
    );
    let mut backup = None;
    for _ in 0..100 {
        backup = std::env::temp_dir()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with(&format!("{service}_rotation-"))
                })
            });
        if backup.is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let backup = backup.expect("rotate control created a timestamped backup");
    let previous = std::fs::read(&backup).expect("rotated log is readable");
    assert!(
        !previous.is_empty(),
        "rotation backup did not retain previous output"
    );
    let mut resumed = false;
    for _ in 0..100 {
        resumed = std::fs::metadata(&log_path).is_ok_and(|metadata| metadata.len() > 0);
        if resumed {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        resumed,
        "application output did not resume after live rotation"
    );
    let current_file_identity = file_identity(&log_path);
    assert_eq!(
        current_file_identity == original_file_identity,
        copy_and_truncate,
        "live rotation did not preserve the expected active file identity: before={original_file_identity:?}, after={current_file_identity:?}"
    );
    let stopped = cli(&["stop", &service]);
    assert!(
        stopped.status.success(),
        "stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
}

struct FileCleanup(Vec<String>);
impl Drop for FileCleanup {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

struct DirectoryCleanup(String);
impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct RotationCleanup {
    active: String,
    prefix: String,
}
impl Drop for RotationCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.active);
        if let Some(directory) = std::path::Path::new(&self.active).parent()
            && let Ok(entries) = directory.read_dir()
        {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&self.prefix)
                {
                    if entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                        let _ = std::fs::remove_dir_all(entry.path());
                    } else {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}
