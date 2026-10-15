use nssm_r::{
    Command, ExitAction, edit_environment, parse_args, quote_windows_arg, split_windows_args,
};

fn args(s: &[&str]) -> Vec<String> {
    s.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn parses_install_arguments_without_losing_spaces() {
    assert_eq!(
        parse_args(&args(&[
            "install",
            "My Service",
            "C:\\Program Files\\app.exe",
            "--mode",
            "a b"
        ]))
        .unwrap(),
        Command::Install {
            service: "My Service".into(),
            application: Some("C:\\Program Files\\app.exe".into()),
            args: vec!["--mode".into(), "a b".into()]
        }
    );
}

#[test]
fn parses_query_and_set_variants() {
    assert_eq!(
        parse_args(&args(&["get", "svc", "AppExit", "2"])).unwrap(),
        Command::Get {
            service: "svc".into(),
            parameter: "AppExit".into(),
            extra: vec!["2".into()]
        }
    );
    assert_eq!(
        parse_args(&args(&["set", "svc", "AppEnvironment", "+TEMP=C:\\tmp"])).unwrap(),
        Command::Set {
            service: "svc".into(),
            parameter: "AppEnvironment".into(),
            values: vec!["+TEMP=C:\\tmp".into()]
        }
    );
    assert_eq!(
        parse_args(&args(&["reset", "svc", "AppExit", "2"])).unwrap(),
        Command::Reset {
            service: "svc".into(),
            parameter: "AppExit".into(),
            extra: vec!["2".into()]
        }
    );
}

#[test]
fn parser_reports_missing_arguments_and_unknown_commands() {
    assert!(parse_args(&args(&["install"])).is_err());
    assert!(parse_args(&args(&["get", "svc"])).is_err());
    assert!(
        parse_args(&args(&["bogus"]))
            .unwrap_err()
            .to_string()
            .contains("unknown command")
    );
}

#[test]
fn parses_remove_list_and_dump_variants() {
    assert_eq!(
        parse_args(&args(&["remove", "svc", "confirm"])).unwrap(),
        Command::Remove {
            service: "svc".into(),
            confirm: true
        }
    );
    assert_eq!(
        parse_args(&args(&["list", "all"])).unwrap(),
        Command::List { all: true }
    );
    assert_eq!(
        parse_args(&args(&["dump", "svc", "copy"])).unwrap(),
        Command::Dump {
            service: "svc".into(),
            new_name: Some("copy".into())
        }
    );
    assert_eq!(
        parse_args(&args(&["list", "unexpected"])).unwrap(),
        Command::List { all: false }
    );
    assert_eq!(
        parse_args(&args(&["list", "all", "ignored"])).unwrap(),
        Command::List { all: true }
    );
}

#[test]
fn parses_multiple_processes_service_names() {
    assert_eq!(
        parse_args(&args(&["processes", "first", "second"])).unwrap(),
        Command::Processes(vec!["first".into(), "second".into()])
    );
    assert!(parse_args(&args(&["processes"])).is_err());
}

#[test]
fn parses_remaining_cli_controls_and_unset_alias() {
    for (command, expected) in [
        ("start", Command::Start("svc".into())),
        ("stop", Command::Stop("svc".into())),
        ("restart", Command::Restart("svc".into())),
        ("pause", Command::Pause("svc".into())),
        ("continue", Command::Continue("svc".into())),
        ("status", Command::Status("svc".into())),
        ("statuscode", Command::StatusCode("svc".into())),
        ("rotate", Command::Rotate("svc".into())),
    ] {
        assert_eq!(parse_args(&args(&[command, "svc"])).unwrap(), expected);
    }
    assert_eq!(
        parse_args(&args(&["unset", "svc", "AppStdout"])).unwrap(),
        Command::Unset {
            service: "svc".into(),
            parameter: "AppStdout".into(),
            extra: vec![]
        }
    );
}

#[test]
fn applies_environment_overrides_additions_and_conditional_removals() {
    let original = args(&["TEMP=C:\\Temp", "PATH=base"]);
    let out = edit_environment(&original, &args(&["+temp=C:\\tmp", "NEW=yes"])).unwrap();
    let unchanged = edit_environment(&out, &args(&["-PATH=wrong"])).unwrap();
    assert_eq!(unchanged, out);
    let out = edit_environment(&unchanged, &args(&["-PATH"])).unwrap();
    assert_eq!(out, args(&["temp=C:\\tmp", "NEW=yes"]));
}

#[test]
fn reset_environment_operator_replaces_the_block() {
    assert_eq!(
        edit_environment(&args(&["A=1", "B=2"]), &args(&[":C=3"])).unwrap(),
        args(&["C=3"])
    );
    assert_eq!(
        edit_environment(&args(&["A=1", "B=2"]), &args(&["C=3", "D=4"])).unwrap(),
        args(&["C=3", "D=4"])
    );
}

#[test]
fn environment_additions_require_assignments_while_removals_accept_a_key() {
    assert!(edit_environment(&[], &args(&["+TEMP"])).is_err());
    assert_eq!(
        edit_environment(&args(&["TEMP=1"]), &args(&["-TEMP"])).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn validates_exit_actions_case_insensitively() {
    assert_eq!(ExitAction::parse("rEsTaRt").unwrap(), ExitAction::Restart);
    assert!(ExitAction::parse("retry").is_err());
}

#[test]
fn windows_argument_quoting_round_trips_spaces_quotes_and_trailing_backslashes() {
    let original = args(&[
        "plain",
        "two words",
        "",
        "quote\"inside",
        "C:\\path with space\\",
    ]);
    let encoded = original
        .iter()
        .map(|arg| quote_windows_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(split_windows_args(&encoded), original);
}
