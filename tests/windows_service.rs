#![cfg(windows)]

use std::{
    io::Write,
    os::windows::io::AsRawHandle,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

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
    let binary = std::env::var_os("NSSM_R_TEST_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_nssm-r").into());
    Command::new(binary)
        .args(args)
        .output()
        .expect("nssm-r executable starts")
}

fn cli_with_input(args: &[&str], input: &str) -> Output {
    let binary = std::env::var_os("NSSM_R_TEST_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_nssm-r").into());
    let mut child = Command::new(binary)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("nssm-r starts with piped input");
    child
        .stdin
        .take()
        .expect("child stdin is piped")
        .write_all(input.as_bytes())
        .expect("confirmation is written");
    child.wait_with_output().expect("nssm-r exits")
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

#[test]
fn installs_runs_stops_and_removes_a_real_scm_service() {
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

    assert!(
        cli(&["set", &service, "Description", "nssm-r test service"])
            .status
            .success()
    );
    let description = cli(&["get", &service, "Description"]);
    assert!(description.status.success());
    assert_eq!(
        String::from_utf8_lossy(&description.stdout).trim(),
        "nssm-r test service"
    );
    assert!(cli(&["reset", &service, "Description"]).status.success());

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
    assert!(
        cli(&["reset", &service, "DependOnService"])
            .status
            .success()
    );

    let managed = cli(&["list"]);
    assert!(managed.status.success());
    assert!(
        String::from_utf8_lossy(&managed.stdout)
            .lines()
            .any(|line| line == service)
    );
    let all = cli(&["list", "all"]);
    assert!(all.status.success());
    assert!(
        String::from_utf8_lossy(&all.stdout)
            .lines()
            .any(|line| line == service)
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
    assert!(dump.contains("set \"NssmR Dump Copy\" Type SERVICE_WIN32_OWN_PROCESS"));

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
    let status = cli(&["status", &service]);
    assert!(status.status.success());
    assert_eq!(
        String::from_utf8_lossy(&status.stdout).trim(),
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
        event_found = Command::new(&powershell)
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
        event_found = Command::new(&powershell)
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
        event_found = Command::new(&powershell)
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
        event_found = Command::new(&powershell)
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
        events_found = Command::new(&powershell)
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
    let fixture = std::env::var_os("NSSM_R_TEST_FIXTURE")
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
    let started = cli(&["start", &service]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(wait_for_service_state(&service, "SERVICE_RUNNING"));

    let mut process_handles = Vec::new();
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
        stdout.contains("existing-stdout"),
        "stdout append mode truncated existing data: {stdout:?}"
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
        "echo first-marker & echo second-marker",
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
        } else {
            if event_action == "Start/Pre" {
                format!(
                    "cmd.exe /c echo {marker}>{path} & cd > {cwd_marker} & set NSSM_ > {abi_marker}"
                )
            } else if event_action == "Exit/Post" {
                format!("cmd.exe /c echo {marker}>{path} & set NSSM_ > {exit_abi_marker}")
            } else {
                format!("cmd.exe /c echo {marker}>{path}")
            }
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
    assert!(cli(&["set", &service, "AppThrottle", "0"]).status.success());
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
    assert_eq!(
        String::from_utf8_lossy(&cli(&["status", &service]).stdout).trim(),
        "SERVICE_RUNNING"
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
        cli(&["set", &service, "AppRestartDelay", "10000"])
            .status
            .success()
    );
    assert!(cli(&["start", &service]).status.success());
    assert!(
        wait_for_service_state(&service, "SERVICE_PAUSED"),
        "service did not enter the NSSM restart-throttle paused state"
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
    let contents = std::fs::read_to_string(backup).expect("rotated startup log can be read");
    assert!(contents.contains("previous-run-marker"));
    assert!(!contents.contains("current-run-marker"));
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
        if let Some(directory) = std::path::Path::new(&self.active).parent() {
            if let Ok(entries) = directory.read_dir() {
                for entry in entries.flatten() {
                    if entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(&self.prefix)
                    {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}
