//! Core, platform-independent NSSM-compatible command and configuration model.
//!
//! Windows service control is implemented in the binary's platform backend. Keeping
//! parsing and configuration semantics here makes them testable on every host.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
#[path = "build_date.rs"]
mod build_date;

/// Returns whether SCM requires a password for this service account.
/// Local identities, virtual service accounts, and managed service accounts
/// are passwordless.
pub fn object_account_requires_password(account: &str, service_name: &str) -> bool {
    let normalized = account.trim().to_ascii_lowercase();
    !matches!(
        normalized.as_str(),
        "localsystem"
            | "system"
            | "nt authority\\system"
            | "localservice"
            | "local service"
            | "nt authority\\localservice"
            | "networkservice"
            | "network service"
            | "nt authority\\networkservice"
    ) && !normalized.eq_ignore_ascii_case(&format!("nt service\\{service_name}"))
        && !normalized.ends_with('$')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    String,
    Dword,
    MultiString,
    ExitAction,
    Hook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingSpec {
    pub name: &'static str,
    pub kind: SettingKind,
    pub default: Option<&'static str>,
}

pub const MANAGED_SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        name: "Application",
        kind: SettingKind::String,
        default: None,
    },
    SettingSpec {
        name: "AppParameters",
        kind: SettingKind::String,
        default: Some(""),
    },
    SettingSpec {
        name: "AppDirectory",
        kind: SettingKind::String,
        default: Some(""),
    },
    SettingSpec {
        name: "AppExit",
        kind: SettingKind::ExitAction,
        default: Some("Restart"),
    },
    SettingSpec {
        name: "AppEvents",
        kind: SettingKind::Hook,
        default: Some(""),
    },
    SettingSpec {
        name: "AppAffinity",
        kind: SettingKind::String,
        default: Some("All"),
    },
    SettingSpec {
        name: "AppEnvironment",
        kind: SettingKind::MultiString,
        default: None,
    },
    SettingSpec {
        name: "AppEnvironmentExtra",
        kind: SettingKind::MultiString,
        default: None,
    },
    SettingSpec {
        name: "AppNoConsole",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppPriority",
        kind: SettingKind::Dword,
        default: Some("32"),
    },
    SettingSpec {
        name: "AppRestartDelay",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppStdin",
        kind: SettingKind::String,
        default: None,
    },
    SettingSpec {
        name: "AppStdinShareMode",
        kind: SettingKind::Dword,
        default: Some("2"),
    },
    SettingSpec {
        name: "AppStdinCreationDisposition",
        kind: SettingKind::Dword,
        default: Some("3"),
    },
    SettingSpec {
        name: "AppStdinFlagsAndAttributes",
        kind: SettingKind::Dword,
        default: Some("128"),
    },
    SettingSpec {
        name: "AppStdout",
        kind: SettingKind::String,
        default: None,
    },
    SettingSpec {
        name: "AppStdoutShareMode",
        kind: SettingKind::Dword,
        default: Some("3"),
    },
    SettingSpec {
        name: "AppStdoutCreationDisposition",
        kind: SettingKind::Dword,
        default: Some("4"),
    },
    SettingSpec {
        name: "AppStdoutFlagsAndAttributes",
        kind: SettingKind::Dword,
        default: Some("128"),
    },
    SettingSpec {
        name: "AppStdoutCopyAndTruncate",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppStderr",
        kind: SettingKind::String,
        default: None,
    },
    SettingSpec {
        name: "AppStderrShareMode",
        kind: SettingKind::Dword,
        default: Some("3"),
    },
    SettingSpec {
        name: "AppStderrCreationDisposition",
        kind: SettingKind::Dword,
        default: Some("4"),
    },
    SettingSpec {
        name: "AppStderrFlagsAndAttributes",
        kind: SettingKind::Dword,
        default: Some("128"),
    },
    SettingSpec {
        name: "AppStderrCopyAndTruncate",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppStopMethodSkip",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppStopMethodConsole",
        kind: SettingKind::Dword,
        default: Some("1500"),
    },
    SettingSpec {
        name: "AppStopMethodWindow",
        kind: SettingKind::Dword,
        default: Some("1500"),
    },
    SettingSpec {
        name: "AppStopMethodThreads",
        kind: SettingKind::Dword,
        default: Some("1500"),
    },
    SettingSpec {
        name: "AppKillProcessTree",
        kind: SettingKind::Dword,
        default: Some("1"),
    },
    SettingSpec {
        name: "AppThrottle",
        kind: SettingKind::Dword,
        default: Some("1500"),
    },
    SettingSpec {
        name: "AppRedirectHook",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppRotateFiles",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppRotateOnline",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppRotateSeconds",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppRotateBytes",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppRotateBytesHigh",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppRotateDelay",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
    SettingSpec {
        name: "AppTimestampLog",
        kind: SettingKind::Dword,
        default: Some("0"),
    },
];

pub fn managed_setting(name: &str) -> Option<&'static SettingSpec> {
    MANAGED_SETTINGS
        .iter()
        .find(|spec| spec.name.eq_ignore_ascii_case(name))
}

/// Quote one argument using the Windows command-line rules used by the C runtime.
pub fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| c.is_whitespace() || c == '"') {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    let mut slashes = 0;
    for ch in arg.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            out.push_str(&"\\".repeat(slashes * 2 + 1));
            out.push('"');
        } else {
            out.push_str(&"\\".repeat(slashes));
            out.push(ch);
        }
        slashes = 0;
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

/// Split an argument string according to Windows quote/backslash rules.
pub fn split_windows_args(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut args = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i == chars.len() {
            break;
        }
        let mut arg = String::new();
        let mut quoted = false;
        while i < chars.len() {
            if chars[i].is_whitespace() && !quoted {
                break;
            }
            let mut slashes = 0;
            while i < chars.len() && chars[i] == '\\' {
                slashes += 1;
                i += 1;
            }
            if i < chars.len() && chars[i] == '"' {
                arg.push_str(&"\\".repeat(slashes / 2));
                if slashes % 2 == 1 {
                    arg.push('"');
                    i += 1;
                } else if quoted && chars.get(i + 1) == Some(&'"') {
                    arg.push('"');
                    i += 2;
                } else {
                    quoted = !quoted;
                    i += 1;
                }
            } else {
                arg.push_str(&"\\".repeat(slashes));
                if i < chars.len() && (quoted || !chars[i].is_whitespace()) {
                    arg.push(chars[i]);
                    i += 1;
                }
            }
        }
        args.push(arg);
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
    }
    args
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Install {
        service: String,
        application: Option<String>,
        args: Vec<String>,
    },
    Remove {
        service: String,
        confirm: bool,
    },
    Start(String),
    Stop(String),
    Restart(String),
    Pause(String),
    Continue(String),
    Status(String),
    StatusCode(String),
    Get {
        service: String,
        parameter: String,
        extra: Vec<String>,
    },
    Set {
        service: String,
        parameter: String,
        values: Vec<String>,
    },
    Reset {
        service: String,
        parameter: String,
        extra: Vec<String>,
    },
    Unset {
        service: String,
        parameter: String,
        extra: Vec<String>,
    },
    Rotate(String),
    List {
        all: bool,
    },
    Dump {
        service: String,
        new_name: Option<String>,
    },
    Processes(Vec<String>),
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);
impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ParseError {}

/// Parse NSSM's command-line shape. Windows command-line tokenization is performed by
/// the OS before this function is called, as it is for the original executable.
pub fn parse_args(args: &[String]) -> Result<Command, ParseError> {
    let Some(op) = args.first() else {
        return Ok(Command::Help);
    };
    let rest = &args[1..];
    if op.eq_ignore_ascii_case("help") || op == "-h" || op == "--help" {
        return Ok(Command::Help);
    }
    if op.eq_ignore_ascii_case("version") || op == "-v" || op == "--version" {
        return Ok(Command::Version);
    }
    let need_service = || {
        rest.first()
            .cloned()
            .ok_or_else(|| ParseError(format!("{op}: service name required")))
    };
    match op.to_ascii_lowercase().as_str() {
        "install" => {
            let service = need_service()?;
            Ok(Command::Install {
                service,
                application: rest.get(1).cloned(),
                args: rest.get(2..).unwrap_or_default().to_vec(),
            })
        }
        "remove" => {
            let service = need_service()?;
            Ok(Command::Remove {
                service,
                confirm: rest
                    .get(1)
                    .is_some_and(|x| x.eq_ignore_ascii_case("confirm")),
            })
        }
        "start" | "stop" | "restart" | "pause" | "continue" | "status" | "statuscode"
        | "rotate" => {
            let service = need_service()?;
            Ok(match op.to_ascii_lowercase().as_str() {
                "start" => Command::Start(service),
                "stop" => Command::Stop(service),
                "restart" => Command::Restart(service),
                "pause" => Command::Pause(service),
                "continue" => Command::Continue(service),
                "status" => Command::Status(service),
                "statuscode" => Command::StatusCode(service),
                "rotate" => Command::Rotate(service),
                _ => Command::Rotate(service),
            })
        }
        "processes" => {
            if rest.is_empty() {
                return Err(ParseError(
                    "processes: at least one service name is required".into(),
                ));
            }
            Ok(Command::Processes(rest.to_vec()))
        }
        "dump" => {
            let service = need_service()?;
            Ok(Command::Dump {
                service,
                new_name: rest.get(1).cloned(),
            })
        }
        "list" => {
            if rest.len() > 1 || rest.first().is_some_and(|x| !x.eq_ignore_ascii_case("all")) {
                return Err(ParseError(
                    "list accepts only the optional argument 'all'".into(),
                ));
            }
            Ok(Command::List {
                all: rest.first().is_some_and(|x| x.eq_ignore_ascii_case("all")),
            })
        }
        "get" | "set" | "reset" | "unset" => {
            if rest.len() < 2 {
                return Err(ParseError(format!(
                    "{op}: service name and parameter required"
                )));
            }
            let service = rest[0].clone();
            let parameter = rest[1].clone();
            let values = rest[2..].to_vec();
            Ok(match op.to_ascii_lowercase().as_str() {
                "get" => Command::Get {
                    service,
                    parameter,
                    extra: values,
                },
                "set" => Command::Set {
                    service,
                    parameter,
                    values,
                },
                "unset" => Command::Unset {
                    service,
                    parameter,
                    extra: values,
                },
                _ => Command::Reset {
                    service,
                    parameter,
                    extra: values,
                },
            })
        }
        _ => Err(ParseError(format!("unknown command: {op}"))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitAction {
    Restart,
    Ignore,
    Exit,
    Suicide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopMethod {
    Console,
    Window,
    Threads,
    Terminate,
}

pub fn stop_methods(skip_mask: u32, no_console: bool) -> Vec<StopMethod> {
    let mut methods = Vec::with_capacity(4);
    if skip_mask & 1 == 0 && !no_console {
        methods.push(StopMethod::Console);
    }
    if skip_mask & 2 == 0 {
        methods.push(StopMethod::Window);
    }
    if skip_mask & 4 == 0 {
        methods.push(StopMethod::Threads);
    }
    if skip_mask & 8 == 0 {
        methods.push(StopMethod::Terminate);
    }
    methods
}
impl ExitAction {
    pub fn parse(s: &str) -> Result<Self, ParseError> {
        match s.to_ascii_lowercase().as_str() {
            "restart" => Ok(Self::Restart),
            "ignore" => Ok(Self::Ignore),
            "exit" => Ok(Self::Exit),
            "suicide" => Ok(Self::Suicide),
            _ => Err(ParseError(format!("invalid AppExit action: {s}"))),
        }
    }
}

/// Resolve the action for an application exit. Per-exit-code rules take priority
/// over the configured default, with NSSM's default of restarting when absent.
pub fn resolve_exit_action(
    code: u32,
    per_code: &BTreeMap<u32, ExitAction>,
    default: Option<ExitAction>,
) -> (ExitAction, bool) {
    if let Some(action) = per_code.get(&code) {
        (*action, false)
    } else {
        (default.unwrap_or(ExitAction::Restart), true)
    }
}

/// NSSM treats a successful exit under an inherited Suicide action as a clean
/// service stop, avoiding an unnecessary failure report for the default case.
pub fn normalize_exit_action(code: u32, action: ExitAction, is_default: bool) -> ExitAction {
    if code == 0 && is_default && action == ExitAction::Suicide {
        ExitAction::Exit
    } else {
        action
    }
}

/// Compute NSSM's exponential restart throttle from its throttle counter. A
/// counter of zero means the next restart is immediate; later retries wait
/// 2, 4, 8, ... up to 128 seconds. The configured fixed delay is a lower bound.
pub fn restart_wait_ms(throttle_counter: u32, restart_delay_ms: u32) -> u32 {
    let backoff = if throttle_counter == 0 {
        0
    } else {
        let exp = throttle_counter.saturating_add(1).min(8) - 1;
        1000_u32 << exp
    };
    backoff.max(restart_delay_ms)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartThrottle {
    counter: u32,
    restart_delay_ms: u32,
}

impl RestartThrottle {
    pub fn new(restart_delay_ms: u32) -> Self {
        Self {
            counter: 0,
            restart_delay_ms,
        }
    }

    /// A child that survives the configured throttle window resets backoff.
    pub fn mark_stable(&mut self) {
        self.counter = 0;
    }

    /// Return the delay before a retry, then advance the consecutive-failure count.
    pub fn after_exit(&mut self) -> u32 {
        if self.counter == 0 && self.restart_delay_ms > 0 {
            self.counter = 1;
        }
        let delay = restart_wait_ms(self.counter, self.restart_delay_ms);
        self.counter = self.counter.saturating_add(1);
        delay
    }

    /// Current consecutive failure count, exposed to lifecycle hooks.
    pub fn counter(&self) -> u32 {
        self.counter
    }
}

/// Apply NSSM's KEY=VALUE / +KEY=VALUE / -KEY[=VALUE] environment-block edits.
/// Existing key order is preserved; new keys are appended in call order.
pub fn edit_environment(existing: &[String], edits: &[String]) -> Result<Vec<String>, ParseError> {
    let Some(first) = edits.first() else {
        return Ok(Vec::new());
    };
    let (operation, first_value) = match first.chars().next() {
        Some('+') => ('+', &first[1..]),
        Some('-') => ('-', &first[1..]),
        Some(':') => (':', &first[1..]),
        _ => ('=', first.as_str()),
    };
    let mut values = Vec::with_capacity(edits.len());
    values.push(first_value);
    values.extend(edits.iter().skip(1).map(String::as_str));
    let mut environment = if operation == '+' || operation == '-' {
        existing.to_vec()
    } else {
        Vec::new()
    };
    for raw in values {
        if operation == '-' {
            if raw.is_empty() || raw.starts_with('=') {
                return Err(ParseError(
                    "environment variable name cannot be empty".into(),
                ));
            }
            let (key, expected_value) = raw
                .split_once('=')
                .map_or((raw, None), |(key, value)| (key, Some(value)));
            if let Some(index) = environment.iter().position(|entry| {
                entry.split_once('=').is_some_and(|(current, value)| {
                    current.eq_ignore_ascii_case(key)
                        && expected_value.is_none_or(|expected| expected == value)
                })
            }) {
                environment.remove(index);
            }
            continue;
        }
        let (key, value) = raw
            .split_once('=')
            .ok_or_else(|| ParseError(format!("environment entry must use KEY=VALUE: {raw}")))?;
        if key.is_empty() {
            return Err(ParseError(
                "environment variable name cannot be empty".into(),
            ));
        }
        let entry = format!("{key}={value}");
        if let Some(index) = environment.iter().position(|current| {
            current
                .split_once('=')
                .is_some_and(|(current_key, _)| current_key.eq_ignore_ascii_case(key))
        }) {
            environment[index] = entry;
        } else {
            environment.push(entry);
        }
    }
    Ok(environment)
}

/// Apply NSSM's service dependency list operators. Group dependencies are stored
/// with SCM's leading `+` marker; callers pass them without that marker.
pub fn edit_dependencies(
    existing: &[String],
    edits: &[String],
    groups: bool,
) -> Result<Vec<String>, ParseError> {
    if edits.is_empty() {
        return Ok(Vec::new());
    }
    let canonical = |value: &str| {
        let value = value.trim_start_matches('+');
        if groups {
            format!("+{value}")
        } else {
            value.to_owned()
        }
    };
    let mut result = existing
        .iter()
        .map(|value| canonical(value))
        .filter(|value| value.len() > usize::from(groups))
        .collect::<Vec<_>>();
    let first = edits[0].as_str();
    let (operation, first_name) = if let Some(value) = first.strip_prefix(':') {
        (0, value.strip_prefix('+').unwrap_or(value))
    } else if let Some(value) = first.strip_prefix('+') {
        (1, value)
    } else if let Some(value) = first.strip_prefix('-') {
        (-1, value)
    } else {
        (0, first)
    };
    let mut names = Vec::with_capacity(edits.len());
    names.push(first_name);
    names.extend(edits.iter().skip(1).map(String::as_str));
    if names.iter().any(|name| name.is_empty()) {
        return Err(ParseError("dependency name cannot be empty".into()));
    }
    if operation == 0 {
        result.clear();
    }
    for name in names {
        let name = canonical(name);
        match operation {
            1 => {
                if !result.iter().any(|entry| entry.eq_ignore_ascii_case(&name)) {
                    result.push(name);
                }
            }
            -1 => result.retain(|entry| !entry.eq_ignore_ascii_case(&name)),
            _ => result.push(name),
        }
    }
    Ok(result)
}

pub fn expand_environment_value(value: &str, entries: &[String]) -> String {
    let mut output = String::new();
    let mut remainder = value;
    while let Some(start) = remainder.find('%') {
        output.push_str(&remainder[..start]);
        let after = &remainder[start + 1..];
        let Some(end) = after.find('%') else {
            output.push_str(&remainder[start..]);
            return output;
        };
        let name = &after[..end];
        if let Some((_, replacement)) = entries
            .iter()
            .filter_map(|entry| entry.split_once('='))
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
        {
            output.push_str(replacement);
        } else {
            output.push('%');
            output.push_str(name);
            output.push('%');
        }
        remainder = &after[end + 1..];
    }
    output.push_str(remainder);
    output
}

pub fn log_rotation_due(
    size_bytes: u64,
    age: Duration,
    minimum_age: Duration,
    minimum_size: u64,
    force: bool,
) -> bool {
    force || (age >= minimum_age && size_bytes >= minimum_size)
}

/// Construct NSSM's timestamped log backup name from the file's last-write time.
pub fn rotated_log_path(path: &Path, modified: SystemTime) -> Option<PathBuf> {
    let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;
    let seconds = since_epoch.as_secs();
    let days = (seconds / 86_400) as i64;
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let millis = since_epoch.subsec_millis();
    let file_name = path.file_name()?.to_string_lossy();
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy());
    let stem = path.file_stem().map(|stem| stem.to_string_lossy())?;
    let rotated = match extension {
        Some(extension) => format!(
            "{stem}-{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}.{millis:03}.{extension}"
        ),
        None => format!(
            "{file_name}-{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}.{millis:03}"
        ),
    };
    Some(path.with_file_name(rotated))
}

pub fn compose_child_environment(
    inherited: &[String],
    replacement: Option<&[String]>,
    extra: &[String],
) -> Result<Vec<String>, ParseError> {
    fn apply(entries: &mut Vec<String>, changes: &[String]) -> Result<(), ParseError> {
        for item in changes {
            let (name, value) = item.split_once('=').ok_or_else(|| {
                ParseError(format!("environment entry must use KEY=VALUE: {item}"))
            })?;
            if name.is_empty() {
                return Err(ParseError(
                    "environment variable name cannot be empty".into(),
                ));
            }
            let value = expand_environment_value(value, entries);
            let updated = format!("{name}={value}");
            if let Some(old) = entries.iter().position(|entry| {
                entry
                    .split_once('=')
                    .is_some_and(|(key, _)| key.eq_ignore_ascii_case(name))
            }) {
                entries[old] = updated;
            } else {
                entries.push(updated);
            }
        }
        Ok(())
    }

    let mut environment = if replacement.is_some() {
        Vec::new()
    } else {
        inherited.to_vec()
    };
    if let Some(replacement) = replacement {
        apply(&mut environment, replacement)?;
    }
    apply(&mut environment, extra)?;
    Ok(environment)
}

/// Convert the `AppAffinity` CPU list syntax (`0-2,4`) to a 64-bit mask.
pub fn parse_cpu_affinity(value: &str) -> Result<u64, ParseError> {
    if value.eq_ignore_ascii_case("all") || value.is_empty() {
        return Ok(0);
    }
    let mut mask = 0u64;
    for part in value.split(',') {
        if part.is_empty() {
            return Err(ParseError("CPU affinity contains an empty item".into()));
        }
        let mut ends = part.split('-');
        let start = ends
            .next()
            .unwrap()
            .parse::<u32>()
            .map_err(|_| ParseError(format!("invalid CPU affinity item: {part}")))?;
        let end = match ends.next() {
            Some(end) => end
                .parse::<u32>()
                .map_err(|_| ParseError(format!("invalid CPU affinity range: {part}")))?,
            None => start,
        };
        if ends.next().is_some() || start > end || end >= 64 {
            return Err(ParseError(format!("CPU affinity range is invalid: {part}")));
        }
        for cpu in start..=end {
            mask |= 1u64 << cpu;
        }
    }
    Ok(mask)
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn explicit_exit_code_policy_overrides_default_and_tracks_fallback() {
        let rules = BTreeMap::from([(17, ExitAction::Ignore)]);
        assert_eq!(
            resolve_exit_action(17, &rules, Some(ExitAction::Exit)),
            (ExitAction::Ignore, false)
        );
        assert_eq!(
            resolve_exit_action(9, &rules, Some(ExitAction::Exit)),
            (ExitAction::Exit, true)
        );
        assert_eq!(
            resolve_exit_action(9, &rules, None),
            (ExitAction::Restart, true)
        );
    }

    #[test]
    fn successful_default_suicide_becomes_a_clean_exit_only_when_inherited() {
        assert_eq!(
            normalize_exit_action(0, ExitAction::Suicide, true),
            ExitAction::Exit
        );
        assert_eq!(
            normalize_exit_action(0, ExitAction::Suicide, false),
            ExitAction::Suicide
        );
        assert_eq!(
            normalize_exit_action(1, ExitAction::Suicide, true),
            ExitAction::Suicide
        );
    }

    #[test]
    fn restart_backoff_is_immediate_then_doubles_to_128_seconds() {
        assert_eq!(restart_wait_ms(0, 0), 0);
        assert_eq!(restart_wait_ms(1, 0), 2000);
        assert_eq!(restart_wait_ms(2, 0), 4000);
        assert_eq!(restart_wait_ms(99, 0), 128_000);
        assert_eq!(restart_wait_ms(u32::MAX, 0), 128_000);
        assert_eq!(restart_wait_ms(1, 5000), 5000);
    }

    #[test]
    fn a_stable_run_resets_backoff_and_fixed_delay_remains_a_floor() {
        let mut throttle = RestartThrottle::new(0);
        assert_eq!(throttle.after_exit(), 0);
        assert_eq!(throttle.after_exit(), 2000);
        assert_eq!(throttle.after_exit(), 4000);
        throttle.mark_stable();
        assert_eq!(throttle.after_exit(), 0);

        let mut delayed = RestartThrottle::new(5000);
        assert_eq!(delayed.after_exit(), 5000);
        assert_eq!(delayed.after_exit(), 5000);
    }

    #[test]
    fn managed_setting_catalog_covers_runtime_configuration_and_defaults() {
        assert_eq!(
            managed_setting("appdirectory").unwrap().kind,
            SettingKind::String
        );
        assert_eq!(
            managed_setting("AppThrottle").unwrap().default,
            Some("1500")
        );
        assert_eq!(
            managed_setting("AppPriority").unwrap().kind,
            SettingKind::Dword
        );
        assert_eq!(
            managed_setting("AppExit").unwrap().kind,
            SettingKind::ExitAction
        );
        assert_eq!(
            managed_setting("AppEnvironmentExtra").unwrap().kind,
            SettingKind::MultiString
        );
        assert!(managed_setting("UnknownParameter").is_none());
    }

    #[test]
    fn child_environment_replaces_then_appends_with_ordered_expansion() {
        let inherited = vec!["PATH=C:\\Windows".to_owned(), "TEMP=C:\\Temp".to_owned()];
        let replacement = vec![
            "BASE=C:\\bin".to_owned(),
            "TEMP=C:\\Temp".to_owned(),
            "PATH=%BASE%;C:\\App".to_owned(),
        ];
        let extra = vec!["PATH=%PATH%;%TEMP%".to_owned(), "OTHER=%path%".to_owned()];
        let result = compose_child_environment(&inherited, Some(&replacement), &extra).unwrap();
        assert_eq!(
            result,
            vec![
                "BASE=C:\\bin",
                "TEMP=C:\\Temp",
                "PATH=C:\\bin;C:\\App;C:\\Temp",
                "OTHER=C:\\bin;C:\\App;C:\\Temp"
            ]
        );
        assert!(compose_child_environment(&[], None, &["BROKEN".into()]).is_err());
    }

    #[test]
    fn dependency_edits_replace_add_remove_and_canonicalize_groups() {
        let current = vec!["RpcSs".into()];
        assert_eq!(
            edit_dependencies(&current, &["LanmanWorkstation".into()], false).unwrap(),
            vec!["LanmanWorkstation"]
        );
        assert_eq!(
            edit_dependencies(&current, &["+LanmanWorkstation".into()], false).unwrap(),
            vec!["RpcSs", "LanmanWorkstation"]
        );
        assert_eq!(
            edit_dependencies(&current, &["-RpcSs".into()], false).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            edit_dependencies(&[], &[":NetBIOSGroup".into()], true).unwrap(),
            vec!["+NetBIOSGroup"]
        );
        assert!(edit_dependencies(&[], &["+".into()], false).is_err());
    }

    #[test]
    fn log_rotation_thresholds_and_timestamped_names_are_deterministic() {
        assert!(!log_rotation_due(
            50,
            Duration::from_secs(20),
            Duration::from_secs(10),
            100,
            false
        ));
        assert!(!log_rotation_due(
            200,
            Duration::from_secs(5),
            Duration::from_secs(10),
            100,
            false
        ));
        assert!(log_rotation_due(
            200,
            Duration::from_secs(20),
            Duration::from_secs(10),
            100,
            false
        ));
        assert!(log_rotation_due(
            0,
            Duration::ZERO,
            Duration::from_secs(10),
            100,
            true
        ));
        assert_eq!(
            rotated_log_path(Path::new("service.log"), UNIX_EPOCH).unwrap(),
            PathBuf::from("service-19700101T000000.000.log")
        );
        assert_eq!(
            rotated_log_path(Path::new("service"), UNIX_EPOCH).unwrap(),
            PathBuf::from("service-19700101T000000.000")
        );
        assert!(
            rotated_log_path(
                Path::new("service.log"),
                UNIX_EPOCH - Duration::from_secs(1)
            )
            .is_none()
        );
    }

    #[test]
    fn graceful_stop_methods_follow_order_and_skip_mask() {
        use StopMethod::{Console, Terminate, Threads, Window};
        assert_eq!(
            stop_methods(0, false),
            vec![Console, Window, Threads, Terminate]
        );
        assert_eq!(stop_methods(1, false), vec![Window, Threads, Terminate]);
        assert_eq!(stop_methods(0, true), vec![Window, Threads, Terminate]);
        assert_eq!(stop_methods(8, false), vec![Console, Window, Threads]);
        assert!(stop_methods(15, false).is_empty());
    }

    #[test]
    fn cpu_affinity_list_builds_masks_and_rejects_invalid_ranges() {
        assert_eq!(parse_cpu_affinity("0-2,4").unwrap(), 0b1_0111);
        assert_eq!(parse_cpu_affinity("All").unwrap(), 0);
        assert!(parse_cpu_affinity("2-1").is_err());
        assert!(parse_cpu_affinity("0,,2").is_err());
        assert!(parse_cpu_affinity("64").is_err());
    }

    #[test]
    fn object_account_password_rules_match_windows_service_account_types() {
        for account in [
            "LocalSystem",
            "NT AUTHORITY\\SYSTEM",
            "NT AUTHORITY\\LocalService",
            "NetworkService",
            "NT SERVICE\\ExampleSvc",
            "CONTOSO\\worker$",
        ] {
            assert!(
                !object_account_requires_password(account, "ExampleSvc"),
                "{account} should not require a password"
            );
        }
        assert!(object_account_requires_password(
            "CONTOSO\\worker",
            "ExampleSvc"
        ));
        assert!(object_account_requires_password(
            "NT SERVICE\\OtherSvc",
            "ExampleSvc"
        ));
    }
}
