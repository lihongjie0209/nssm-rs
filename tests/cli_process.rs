use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nssm-rs"))
        .args(args)
        .output()
        .expect("binary should launch")
}

#[test]
fn help_and_version_are_successful_commands_with_output() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    assert!(!help.stdout.is_empty());

    let version = run(&["version"]);
    assert!(version.status.success());
    assert!(!version.stdout.is_empty());
}

#[cfg(not(windows))]
#[test]
fn service_operation_on_unsupported_host_fails_without_claiming_success() {
    let result = run(&["status", "temporary-test-service"]);
    assert!(!result.status.success());
    assert!(!result.stderr.is_empty());
}
