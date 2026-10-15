use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nssm-rs"))
        .args(args)
        .output()
        .expect("binary should launch")
}

#[test]
fn help_and_version_are_successful_commands_with_output() {
    let canonical_help = run(&["help"]);
    assert_eq!(canonical_help.status.code(), Some(0));
    assert!(!canonical_help.stdout.is_empty());
    assert!(canonical_help.stderr.is_empty());

    for spelling in ["HELP", "-h", "--help"] {
        let help = run(&[spelling]);
        assert_eq!(help.status.code(), Some(0), "{spelling}");
        assert_eq!(help.stdout, canonical_help.stdout, "{spelling}");
        assert_eq!(help.stderr, canonical_help.stderr, "{spelling}");
    }

    let version = run(&["version"]);
    assert!(version.status.success());
    assert!(!version.stdout.is_empty());
}

#[test]
fn version_aliases_report_the_same_result_as_the_canonical_command() {
    let canonical = run(&["version"]);
    assert_eq!(canonical.status.code(), Some(0));
    assert!(!canonical.stdout.is_empty());
    assert!(canonical.stderr.is_empty());

    for spelling in [
        "/version",
        "/VERSION",
        "-v",
        "-V",
        "-version",
        "-VERSION",
        "--version",
        "--VERSION",
    ] {
        let result = run(&[spelling]);
        assert_eq!(result.status.code(), Some(0), "{spelling}");
        assert_eq!(result.stdout, canonical.stdout, "{spelling}");
        assert_eq!(result.stderr, canonical.stderr, "{spelling}");
    }
}

#[test]
fn no_arguments_print_usage_to_stderr_and_return_usage_exit_status() {
    let result = run(&[]);
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Usage: nssm-rs"));
}

#[cfg(windows)]
#[test]
fn unknown_native_service_setting_lists_supported_parameters() {
    let result = run(&["get", "Schedule", "DefinitelyNotASetting"]);
    assert_eq!(result.status.code(), Some(1));
    assert!(result.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.starts_with(
            "Invalid parameter \"DefinitelyNotASetting\".  Valid parameters are:\r\r\n"
        )
    );
    assert!(stderr.contains("Application\r\n"));
    assert!(stderr.contains("DependOnGroup\r\n"));
    assert!(stderr.ends_with("Type\r\n"));
}

#[cfg(windows)]
#[test]
fn managed_settings_on_native_services_match_nssm_error_bytes() {
    let ordinary = run(&["get", "Spooler", "AppEnvironment"]);
    assert_eq!(ordinary.status.code(), Some(1));
    assert!(ordinary.stdout.is_empty());
    assert_eq!(
        ordinary.stderr,
        b"Parameter \"AppEnvironment\" is only valid for services managed by NSSM!\r\r\n"
    );

    for (setting, expected) in [
        (
            "AppExit",
            b"Parameter \"AppExit\" requires a subparameter!\r\r\n".as_slice(),
        ),
        (
            "AppEvents",
            b"Parameter \"AppEvents\" requires a subparameter!\r\r\n".as_slice(),
        ),
    ] {
        let result = run(&["get", "Spooler", setting]);
        assert_eq!(result.status.code(), Some(1), "{setting}");
        assert!(result.stdout.is_empty(), "{setting}");
        assert_eq!(result.stderr, expected, "{setting}");
    }
}

#[cfg(not(windows))]
#[test]
fn service_operation_on_unsupported_host_fails_without_claiming_success() {
    let result = run(&["status", "temporary-test-service"]);
    assert!(!result.status.success());
    assert!(!result.stderr.is_empty());
}
