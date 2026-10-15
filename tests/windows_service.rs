#![cfg(windows)]

use std::{
    os::windows::io::AsRawHandle,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static SERVICE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_nssm-r"))
        .args(args)
        .output()
        .expect("nssm-r executable starts")
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
    let _files = FileCleanup(event_paths.clone());
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
    for ((event_action, marker), path) in [
        ("Start/Pre", "start-pre %NSSM_EVENT% %NSSM_ACTION% %NSSM_TRIGGER% %NSSM_DEADLINE% %NSSM_SERVICE_DISPLAYNAME% %NSSM_COMMAND_LINE%"),
        ("Start/Post", "start-post"),
        ("Exit/Post", "exit-post"),
    ]
    .into_iter()
    .zip(&event_paths)
    {
        let hook = format!("cmd.exe /c echo {marker}>{path}");
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
            .all(|path| std::path::Path::new(path).is_file());
        if hooks_ran {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        hooks_ran,
        "not all configured lifecycle hooks created their marker files"
    );
    let start_hook = std::fs::read_to_string(&event_paths[0]).unwrap();
    let mut fields = start_hook.split_whitespace();
    assert_eq!(fields.next(), Some("start-pre"));
    assert_eq!(fields.next(), Some("Start"));
    assert_eq!(fields.next(), Some("Pre"));
    assert_eq!(fields.next(), Some("START"));
    assert_eq!(fields.next(), Some("20000"));
    assert_eq!(fields.next(), Some("Hook"));
    assert_eq!(fields.next(), Some("ABITest"));
    assert_eq!(fields.next(), Some("Service"));
    let command_line = fields.collect::<Vec<_>>().join(" ");
    assert!(command_line.contains("cmd.exe"));
    assert_eq!(
        std::fs::read_to_string(&event_paths[1]).unwrap().trim(),
        "start-post"
    );
    assert_eq!(
        std::fs::read_to_string(&event_paths[2]).unwrap().trim(),
        "exit-post"
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
