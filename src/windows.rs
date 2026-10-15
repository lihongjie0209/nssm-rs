use nssm_r::{
    Command, ExitAction, MANAGED_SETTINGS, SettingKind, StopMethod, compose_child_environment,
    edit_dependencies, edit_environment, expand_environment_value, log_rotation_due,
    managed_setting, normalize_exit_action, parse_cpu_affinity, quote_windows_arg,
    rotated_log_path, split_windows_args,
};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt},
    path::Path,
    process::{Child, Command as ProcessCommand, Stdio},
    ptr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_NO_MORE_ITEMS, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, NO_ERROR,
        WPARAM,
    },
    System::Console::{
        AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
    },
    System::{
        Diagnostics::ToolHelp::*,
        EventLog::{
            DeregisterEventSource, EVENTLOG_ERROR_TYPE, RegisterEventSourceW, ReportEventW,
        },
        JobObjects::*,
        Registry::*,
        Services::*,
        Threading::*,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, PBT_APMPOWERSTATUSCHANGE, PBT_APMRESUMEAUTOMATIC,
        PostThreadMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_CLOSE, WM_QUIT,
    },
};

const DELETE_ACCESS: u32 = 0x0001_0000;
const STOP_SKIP_CONSOLE: u32 = 1;
const EVENT_DISPATCHER_FAILED: u32 = 0xc000_03e9;
const EVENT_START_SERVICE_FAILED: u32 = 0xc000_03ee;
const EVENT_STARTED_SERVICE: u32 = 0x4000_03f0;
const EVENT_CREATEPROCESS_FAILED: u32 = 0xc000_03f2;
const EVENT_ENDED_SERVICE: u32 = 0x4000_03f5;
const EVENT_EXIT_RESTART: u32 = 0x4000_03f6;
const EVENT_EXIT_IGNORE: u32 = 0x4000_03f7;
const EVENT_EXIT: u32 = 0x4000_03f8;
const EVENT_EXIT_UNCLEAN: u32 = 0x4000_03fc;
const SERVICE_LAUNCH_FAILURE_CODE: u32 = 3;
const EVENT_SOURCE: &str = "nssm";
const SERVICE_KEY_PREFIX: &str = "SYSTEM\\CurrentControlSet\\Services\\";
static SERVICE_STATUS_HANDLE_VALUE: AtomicIsize = AtomicIsize::new(0);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static ROTATE_REQUESTED: AtomicBool = AtomicBool::new(false);
static ROTATE_ENABLED: AtomicBool = AtomicBool::new(false);
static CONTINUE_REQUESTED: AtomicBool = AtomicBool::new(false);
static POWER_HOOK_REQUESTED: AtomicU32 = AtomicU32::new(0);
static HOOK_START_REQUESTED_COUNT: AtomicU32 = AtomicU32::new(0);
static HOOK_START_COUNT: AtomicU32 = AtomicU32::new(0);
static HOOK_EXIT_COUNT: AtomicU32 = AtomicU32::new(0);
static HOOK_THROTTLE_COUNT: AtomicU32 = AtomicU32::new(0);
static HOOK_LAST_CONTROL: AtomicU32 = AtomicU32::new(0);
static HOOK_SERVICE_START_MS: AtomicU64 = AtomicU64::new(0);
static HOOK_APPLICATION_START_MS: AtomicU64 = AtomicU64::new(0);
static HOOK_APPLICATION_RUNTIME_MS: AtomicU64 = AtomicU64::new(0);
static CHILD_PROCESS: Mutex<Option<ManagedChild>> = Mutex::new(None);
static ASYNC_HOOKS: Mutex<Vec<Child>> = Mutex::new(Vec::new());

struct ManagedChild {
    child: Child,
    job: isize,
}

struct LogDestination {
    path: String,
    sharing: u32,
    disposition: AtomicU32,
    attributes: u32,
    file: Mutex<Option<File>>,
}

impl LogDestination {
    fn new(service: &str, setting: &str, path: String, file: File) -> Result<Arc<Self>, String> {
        let sharing = read_reg_dword(service, &format!("{setting}ShareMode"), 3)?;
        let disposition = read_reg_dword(service, &format!("{setting}CreationDisposition"), 4)?;
        let attributes = read_reg_dword(service, &format!("{setting}FlagsAndAttributes"), 128)?;
        Ok(Arc::new(Self {
            path,
            sharing,
            disposition: AtomicU32::new(disposition),
            attributes,
            file: Mutex::new(Some(file)),
        }))
    }

    fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        let mut file = self.file.lock().map_err(|_| io::ErrorKind::Other)?;
        file.as_mut()
            .ok_or(io::ErrorKind::BrokenPipe)?
            .write_all(bytes)
    }

    fn flush(&self) -> io::Result<()> {
        let mut file = self.file.lock().map_err(|_| io::ErrorKind::Other)?;
        file.as_mut().ok_or(io::ErrorKind::BrokenPipe)?.flush()
    }

    fn rotate(
        &self,
        minimum_age: Duration,
        minimum_size: u64,
        delay: Duration,
        force: bool,
        copy_and_truncate: bool,
    ) -> Result<bool, String> {
        let mut file_guard = self
            .file
            .lock()
            .map_err(|_| "log destination lock poisoned".to_owned())?;
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        let modified = metadata.modified().map_err(|error| error.to_string())?;
        if !log_rotation_due(
            metadata.len(),
            modified.elapsed().unwrap_or_default(),
            minimum_age,
            minimum_size,
            force,
        ) {
            return Ok(false);
        }
        let rotated = rotated_log_path(Path::new(&self.path), std::time::SystemTime::now())
            .ok_or_else(|| format!("could not create rotated name for {}", self.path))?;
        file_guard
            .as_mut()
            .ok_or_else(|| "log destination is closed".to_owned())?
            .flush()
            .map_err(|error| error.to_string())?;

        if copy_and_truncate {
            fs::copy(&self.path, &rotated).map_err(|error| {
                format!(
                    "unable to copy {} to {}: {error}",
                    self.path,
                    rotated.display()
                )
            })?;
            drop(file_guard);
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            let mut file_guard = self
                .file
                .lock()
                .map_err(|_| "log destination lock poisoned".to_owned())?;
            file_guard
                .as_mut()
                .expect("log file remains open while copy-truncating")
                .set_len(0)
                .map_err(|error| error.to_string())?;
        } else {
            drop(file_guard.take());
            if let Err(error) = fs::rename(&self.path, &rotated) {
                self.disposition.store(4, Ordering::SeqCst);
                *file_guard = Some(self.reopen(4).map_err(|reopen| {
                    format!(
                        "unable to move {}: {error}; reopen failed: {reopen}",
                        self.path
                    )
                })?);
                return Err(format!("unable to move {}: {error}", self.path));
            }
            *file_guard = Some(
                self.reopen(self.disposition.load(Ordering::SeqCst))
                    .map_err(|error| {
                        format!("unable to reopen {} after rotation: {error}", self.path)
                    })?,
            );
        }
        Ok(true)
    }

    fn reopen(&self, disposition: u32) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options
            .write(true)
            .append(disposition != 2 && disposition != 5)
            .share_mode(self.sharing)
            .attributes(self.attributes);
        match disposition {
            1 => {
                options.create_new(true);
            }
            2 => {
                options.create(true).truncate(true);
            }
            3 => {}
            4 => {
                options.create(true);
            }
            5 => {
                options.truncate(true);
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid disposition",
                ));
            }
        }
        let mut file = options.open(&self.path)?;
        file.seek(SeekFrom::End(0))?;
        Ok(file)
    }
}

struct RegKey(HKEY);
impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn open_registry_key(
    service: &str,
    relative: &str,
    create: bool,
    access: u32,
) -> Result<RegKey, String> {
    let suffix = if relative.is_empty() {
        String::new()
    } else {
        format!("\\{relative}")
    };
    let full_path = format!("{SERVICE_KEY_PREFIX}{service}\\Parameters{suffix}");
    let path = wide(OsStr::new(&full_path));
    let mut key = ptr::null_mut();
    let code = if create {
        unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                path.as_ptr(),
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                access,
                ptr::null(),
                &mut key,
                ptr::null_mut(),
            )
        }
    } else {
        unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, access, &mut key) }
    };
    if code == 0 {
        Ok(RegKey(key))
    } else {
        Err(registry_error(code))
    }
}

fn open_registry_key_optional(
    service: &str,
    relative: &str,
    access: u32,
) -> Result<Option<RegKey>, String> {
    let suffix = if relative.is_empty() {
        String::new()
    } else {
        format!("\\{relative}")
    };
    let path = wide(OsStr::new(&format!(
        "{SERVICE_KEY_PREFIX}{service}\\Parameters{suffix}"
    )));
    let mut key = ptr::null_mut();
    match unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, access, &mut key) } {
        0 => Ok(Some(RegKey(key))),
        2 => Ok(None),
        code => Err(registry_error(code)),
    }
}

fn read_raw_value(key: HKEY, name: Option<&str>) -> Result<Option<(u32, Vec<u8>)>, String> {
    let name = name.map(|s| wide(OsStr::new(s)));
    let name_ptr = name.as_ref().map_or(ptr::null(), |s| s.as_ptr());
    let mut bytes = 0;
    let mut kind = 0;
    let code = unsafe {
        RegQueryValueExW(
            key,
            name_ptr,
            ptr::null(),
            &mut kind,
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if code == 2 {
        return Ok(None);
    }
    if code != 0 {
        return Err(registry_error(code));
    }
    let mut data = vec![0u8; bytes as usize];
    let code = unsafe {
        RegQueryValueExW(
            key,
            name_ptr,
            ptr::null(),
            &mut kind,
            data.as_mut_ptr(),
            &mut bytes,
        )
    };
    if code == 234 {
        data.resize(bytes as usize, 0);
        let code = unsafe {
            RegQueryValueExW(
                key,
                name_ptr,
                ptr::null(),
                &mut kind,
                data.as_mut_ptr(),
                &mut bytes,
            )
        };
        if code != 0 {
            return Err(registry_error(code));
        }
    } else if code != 0 {
        return Err(registry_error(code));
    }
    data.truncate(bytes as usize);
    Ok(Some((kind, data)))
}

fn read_multi_parameter(service: &str, name: &str) -> Result<Option<Vec<String>>, String> {
    let key = open_registry_key(service, "", false, KEY_QUERY_VALUE)?;
    let Some((kind, bytes)) = read_raw_value(key.0, Some(name))? else {
        return Ok(None);
    };
    if kind != REG_MULTI_SZ {
        return Err(format!("service setting {name} is not REG_MULTI_SZ"));
    }
    Ok(Some(decode_multi_string(&bytes)?))
}

fn write_raw_value(key: HKEY, name: Option<&str>, kind: u32, data: &[u8]) -> Result<(), String> {
    let name = name.map(|s| wide(OsStr::new(s)));
    let name_ptr = name.as_ref().map_or(ptr::null(), |s| s.as_ptr());
    let code = unsafe { RegSetValueExW(key, name_ptr, 0, kind, data.as_ptr(), data.len() as u32) };
    if code == 0 {
        Ok(())
    } else {
        Err(registry_error(code))
    }
}

fn delete_raw_value(key: HKEY, name: Option<&str>) -> Result<(), String> {
    let name = name.map(|s| wide(OsStr::new(s)));
    let name_ptr = name.as_ref().map_or(ptr::null(), |s| s.as_ptr());
    let code = unsafe { RegDeleteValueW(key, name_ptr) };
    if code == 0 || code == 2 {
        Ok(())
    } else {
        Err(registry_error(code))
    }
}

fn encode_wide(value: &str, terminators: usize) -> Vec<u8> {
    value
        .encode_utf16()
        .chain(std::iter::repeat_n(0, terminators))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn decode_wide(data: &[u8]) -> Result<String, String> {
    if data.len() % 2 != 0 {
        return Err("registry string has an odd byte length".into());
    }
    let chars: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|c| *c != 0)
        .collect();
    Ok(String::from_utf16_lossy(&chars))
}

fn decode_multi_string(data: &[u8]) -> Result<Vec<String>, String> {
    if data.len() % 2 != 0 {
        return Err("registry multi-string has an odd byte length".into());
    }
    let chars: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let mut result = Vec::new();
    let mut start = 0;
    for (i, ch) in chars.iter().enumerate() {
        if *ch == 0 {
            if i == start {
                break;
            }
            result.push(String::from_utf16_lossy(&chars[start..i]));
            start = i + 1;
        }
    }
    Ok(result)
}

fn encode_multi_string(values: &[String]) -> Vec<u8> {
    let mut wide_values = Vec::new();
    for value in values {
        wide_values.extend(value.encode_utf16());
        wide_values.push(0);
    }
    wide_values.push(0);
    if wide_values.len() == 1 {
        wide_values.push(0);
    }
    wide_values.into_iter().flat_map(u16::to_le_bytes).collect()
}

fn setting_location(
    service: &str,
    spec: &nssm_r::SettingSpec,
    additional: Option<&str>,
    create: bool,
) -> Result<(RegKey, Option<String>), String> {
    match spec.kind {
        SettingKind::ExitAction => {
            let code =
                additional.ok_or_else(|| "AppExit requires an exit code or Default".to_owned())?;
            let value = if code.eq_ignore_ascii_case("default") || code == "*" || code.is_empty() {
                None
            } else {
                code.parse::<u32>().map_err(|_| {
                    "AppExit code must be an unsigned integer or Default".to_owned()
                })?;
                Some(code.to_owned())
            };
            Ok((
                open_registry_key(
                    service,
                    "AppExit",
                    create,
                    if create {
                        KEY_SET_VALUE
                    } else {
                        KEY_QUERY_VALUE
                    },
                )?,
                value,
            ))
        }
        SettingKind::Hook => {
            let hook =
                additional.ok_or_else(|| "AppEvents requires an event/action pair".to_owned())?;
            let (event, action) = hook
                .split_once('/')
                .ok_or_else(|| "hook must use event/action syntax".to_owned())?;
            if !valid_hook(event, action) {
                return Err("unsupported AppEvents event/action pair".into());
            }
            Ok((
                open_registry_key(
                    service,
                    &format!("AppEvents\\{event}"),
                    create,
                    if create {
                        KEY_SET_VALUE
                    } else {
                        KEY_QUERY_VALUE
                    },
                )?,
                Some(action.into()),
            ))
        }
        _ => Ok((
            open_registry_key(
                service,
                "",
                create,
                if create {
                    KEY_SET_VALUE | KEY_QUERY_VALUE
                } else {
                    KEY_QUERY_VALUE
                },
            )?,
            Some(spec.name.into()),
        )),
    }
}

fn valid_hook(event: &str, action: &str) -> bool {
    match event.to_ascii_lowercase().as_str() {
        "exit" => action.eq_ignore_ascii_case("post"),
        "power" => action.eq_ignore_ascii_case("change") || action.eq_ignore_ascii_case("resume"),
        "rotate" | "start" => {
            action.eq_ignore_ascii_case("pre") || action.eq_ignore_ascii_case("post")
        }
        "stop" => action.eq_ignore_ascii_case("pre"),
        _ => false,
    }
}

fn power_hook_action(event_type: u32) -> Option<u32> {
    match event_type {
        PBT_APMRESUMEAUTOMATIC => Some(2),   // Power/Resume
        PBT_APMPOWERSTATUSCHANGE => Some(1), // Power/Change
        _ => None,
    }
}

fn parse_u32(value: &str) -> Result<u32, String> {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16).map_err(|_| format!("invalid unsigned integer: {value}"))
    } else {
        value
            .parse()
            .map_err(|_| format!("invalid unsigned integer: {value}"))
    }
}

fn priority_value(name: &str) -> Result<u32, String> {
    match name.to_ascii_uppercase().as_str() {
        "IDLE_PRIORITY_CLASS" => Ok(0x40),
        "BELOW_NORMAL_PRIORITY_CLASS" => Ok(0x4000),
        "NORMAL_PRIORITY_CLASS" => Ok(0x20),
        "ABOVE_NORMAL_PRIORITY_CLASS" => Ok(0x8000),
        "HIGH_PRIORITY_CLASS" => Ok(0x80),
        "REALTIME_PRIORITY_CLASS" => Ok(0x100),
        _ => Err(format!("invalid application priority: {name}")),
    }
}

fn priority_name(value: u32) -> Result<&'static str, String> {
    match value {
        0x40 => Ok("IDLE_PRIORITY_CLASS"),
        0x4000 => Ok("BELOW_NORMAL_PRIORITY_CLASS"),
        0x20 => Ok("NORMAL_PRIORITY_CLASS"),
        0x8000 => Ok("ABOVE_NORMAL_PRIORITY_CLASS"),
        0x80 => Ok("HIGH_PRIORITY_CLASS"),
        0x100 => Ok("REALTIME_PRIORITY_CLASS"),
        _ => Err(format!("unknown application priority value: {value}")),
    }
}

fn get_setting(service: &str, parameter: &str, extra: &[String]) -> Result<(), String> {
    let Some(spec) = managed_setting(parameter) else {
        return get_native_setting(service, parameter, extra);
    };
    let additional = extra.first().map(String::as_str);
    if spec.kind == SettingKind::ExitAction {
        let code =
            additional.ok_or_else(|| "AppExit requires an exit code or Default".to_owned())?;
        // NSSM fails this query when the AppExit key has not been initialized.
        let _key = open_registry_key(service, "AppExit", false, KEY_QUERY_VALUE)?;
        let action = if code.eq_ignore_ascii_case("default") || code == "*" || code.is_empty() {
            read_exit_rule(service, None)?.unwrap_or(ExitAction::Restart)
        } else {
            let code = parse_u32(code)?;
            exit_action(service, code)?.0
        };
        let display = match action {
            ExitAction::Restart => "Restart",
            ExitAction::Ignore => "Ignore",
            ExitAction::Exit => "Exit",
            ExitAction::Suicide => "Suicide",
        };
        println!("{display}");
        return Ok(());
    }
    if spec.kind == SettingKind::Hook {
        let hook =
            additional.ok_or_else(|| "AppEvents requires an event/action pair".to_owned())?;
        println!("{}", read_hook(service, hook)?.unwrap_or_default());
        return Ok(());
    }
    let (key, value_name) = setting_location(service, spec, additional, false)?;
    let raw = read_raw_value(key.0, value_name.as_deref())?;
    let display = match spec.kind {
        SettingKind::ExitAction => {
            let code =
                additional.ok_or_else(|| "AppExit requires an exit code or Default".to_owned())?;
            let explicit = if code.eq_ignore_ascii_case("default") || code == "*" || code.is_empty()
            {
                None
            } else {
                raw
            };
            let value = if let Some((_, bytes)) = explicit {
                Some(decode_wide(&bytes)?)
            } else {
                let (default_key, _) = setting_location(service, spec, Some("Default"), false)?;
                read_raw_value(default_key.0, None)?
                    .map(|(_, bytes)| decode_wide(&bytes))
                    .transpose()?
            };
            value.unwrap_or_else(|| "Restart".into())
        }
        SettingKind::Hook => raw
            .map(|(_, bytes)| decode_wide(&bytes))
            .transpose()?
            .unwrap_or_default(),
        SettingKind::MultiString => {
            let values = raw
                .map(|(_, bytes)| decode_multi_string(&bytes))
                .transpose()?
                .unwrap_or_default();
            if let Some(key) = extra.first() {
                values
                    .iter()
                    .find_map(|entry| {
                        entry
                            .split_once('=')
                            .filter(|(name, _)| name.eq_ignore_ascii_case(key))
                            .map(|(_, value)| value.to_owned())
                    })
                    .unwrap_or_default()
            } else {
                values.join("\r\n")
            }
        }
        SettingKind::Dword => {
            let number = raw
                .map(|(_, bytes)| {
                    if bytes.len() != 4 {
                        return Err(format!("setting {parameter} is not a 32-bit number"));
                    }
                    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
                })
                .transpose()?
                .unwrap_or_else(|| {
                    spec.default
                        .and_then(|value| value.parse().ok())
                        .unwrap_or_default()
                });
            if parameter.eq_ignore_ascii_case("AppPriority") {
                priority_name(number)?.into()
            } else {
                number.to_string()
            }
        }
        SettingKind::String => raw
            .map(|(_, bytes)| decode_wide(&bytes))
            .transpose()?
            .or_else(|| spec.default.map(str::to_owned))
            .unwrap_or_default(),
    };
    println!("{display}");
    Ok(())
}

fn set_setting(service: &str, parameter: &str, values: &[String]) -> Result<(), String> {
    let Some(spec) = managed_setting(parameter) else {
        return set_native_setting(service, parameter, values);
    };
    let additional = match spec.kind {
        SettingKind::ExitAction | SettingKind::Hook => values.first().map(String::as_str),
        _ => None,
    };
    let (key, value_name) = setting_location(service, spec, additional, true)?;
    match spec.kind {
        SettingKind::ExitAction => {
            if values.len() != 2 {
                return Err("set AppExit requires an exit code and action".into());
            }
            let action = ExitAction::parse(&values[1]).map_err(|error| error.to_string())?;
            let action = match action {
                ExitAction::Restart => "Restart",
                ExitAction::Ignore => "Ignore",
                ExitAction::Exit => "Exit",
                ExitAction::Suicide => "Suicide",
            };
            write_raw_value(
                key.0,
                value_name.as_deref(),
                REG_SZ,
                &encode_wide(action, 1),
            )
        }
        SettingKind::Hook => {
            let command = values.iter().skip(1).cloned().collect::<Vec<_>>().join(" ");
            if command.is_empty() {
                delete_raw_value(key.0, value_name.as_deref())
            } else {
                write_raw_value(
                    key.0,
                    value_name.as_deref(),
                    REG_EXPAND_SZ,
                    &encode_wide(&command, 1),
                )
            }
        }
        SettingKind::MultiString => {
            if values.is_empty() {
                return delete_raw_value(key.0, value_name.as_deref());
            }
            let existing = read_raw_value(key.0, value_name.as_deref())?
                .map(|(_, bytes)| decode_multi_string(&bytes))
                .transpose()?
                .unwrap_or_default();
            let updated = edit_environment(&existing, values).map_err(|error| error.to_string())?;
            write_raw_value(
                key.0,
                value_name.as_deref(),
                REG_MULTI_SZ,
                &encode_multi_string(&updated),
            )
        }
        SettingKind::Dword => {
            let value = values.join(" ");
            if value.is_empty() {
                return Err(format!("set {parameter} requires a value"));
            }
            let number = if parameter.eq_ignore_ascii_case("AppPriority") {
                priority_value(&value)?
            } else {
                parse_u32(&value)?
            };
            let default = spec.default.and_then(|value| value.parse::<u32>().ok());
            if Some(number) == default {
                delete_raw_value(key.0, value_name.as_deref())
            } else {
                write_raw_value(
                    key.0,
                    value_name.as_deref(),
                    REG_DWORD,
                    &number.to_le_bytes(),
                )
            }
        }
        SettingKind::String => {
            let value = values.join(" ");
            if values.is_empty() {
                return Err(format!("set {parameter} requires a value"));
            }
            let kind = if parameter.eq_ignore_ascii_case("AppAffinity") {
                REG_SZ
            } else {
                REG_EXPAND_SZ
            };
            if (spec
                .default
                .is_some_and(|default| !default.is_empty() && default == value))
                || (parameter.eq_ignore_ascii_case("AppAffinity") && value.is_empty())
            {
                delete_raw_value(key.0, value_name.as_deref())
            } else {
                write_raw_value(key.0, value_name.as_deref(), kind, &encode_wide(&value, 1))
            }
        }
    }
}

fn reset_setting(service: &str, parameter: &str, extra: &[String]) -> Result<(), String> {
    let Some(spec) = managed_setting(parameter) else {
        return reset_native_setting(service, parameter, extra);
    };
    let additional = match spec.kind {
        SettingKind::ExitAction | SettingKind::Hook => extra.first().map(String::as_str),
        _ => None,
    };
    if spec.kind == SettingKind::ExitAction && additional.is_none() {
        return Err("reset AppExit requires an exit code or Default".into());
    }
    let reset_default_action = spec.kind == SettingKind::ExitAction
        && additional.is_some_and(|value| value.eq_ignore_ascii_case("default"));
    let (key, value_name) = setting_location(service, spec, additional, true)?;
    if reset_default_action {
        write_raw_value(key.0, None, REG_SZ, &encode_wide("Restart", 1))
    } else {
        delete_raw_value(key.0, value_name.as_deref())
    }
}

fn open_service_registry(service: &str, create: bool, access: u32) -> Result<RegKey, String> {
    let path = wide(OsStr::new(&format!("{SERVICE_KEY_PREFIX}{service}")));
    let mut key = ptr::null_mut();
    let code = if create {
        unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                path.as_ptr(),
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                access,
                ptr::null(),
                &mut key,
                ptr::null_mut(),
            )
        }
    } else {
        unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, access, &mut key) }
    };
    if code == 0 {
        Ok(RegKey(key))
    } else {
        Err(registry_error(code))
    }
}

fn query_service_config(service: SC_HANDLE) -> Result<(Vec<usize>, QUERY_SERVICE_CONFIGW), String> {
    let mut needed = 0;
    unsafe {
        QueryServiceConfigW(service, ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let mut storage = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        QueryServiceConfigW(
            service,
            storage.as_mut_ptr().cast(),
            (storage.len() * std::mem::size_of::<usize>()) as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().to_string());
    }
    let config = unsafe { *(storage.as_ptr().cast::<QUERY_SERVICE_CONFIGW>()) };
    Ok((storage, config))
}

fn wide_pointer(value: *const u16) -> String {
    if value.is_null() {
        return String::new();
    }
    let mut len = 0;
    while unsafe { *value.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(value, len) })
}

unsafe fn dependency_list(value: *const u16) -> Vec<String> {
    if value.is_null() {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut cursor = value;
    while unsafe { *cursor } != 0 {
        let item = wide_pointer(cursor);
        cursor = unsafe { cursor.add(item.encode_utf16().count() + 1) };
        result.push(item);
    }
    result
}

fn dependency_block(services: &[String], groups: &[String]) -> Vec<u16> {
    let mut result = Vec::new();
    for item in services.iter().chain(groups.iter()) {
        result.extend(item.encode_utf16());
        result.push(0);
    }
    result.push(0);
    if services.is_empty() && groups.is_empty() {
        result.push(0);
    }
    result
}

fn service_description(service: SC_HANDLE) -> Result<String, String> {
    let mut needed = 0;
    unsafe {
        QueryServiceConfig2W(
            service,
            SERVICE_CONFIG_DESCRIPTION,
            ptr::null_mut(),
            0,
            &mut needed,
        );
    }
    if needed == 0 {
        return Ok(String::new());
    }
    let mut storage = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        QueryServiceConfig2W(
            service,
            SERVICE_CONFIG_DESCRIPTION,
            storage.as_mut_ptr().cast(),
            (storage.len() * std::mem::size_of::<usize>()) as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().to_string());
    }
    let description = unsafe { *(storage.as_ptr().cast::<SERVICE_DESCRIPTIONW>()) };
    Ok(wide_pointer(description.lpDescription))
}

fn get_native_setting(service: &str, parameter: &str, extra: &[String]) -> Result<(), String> {
    match parameter.to_ascii_lowercase().as_str() {
        "environment" => {
            let key = open_service_registry(service, false, KEY_QUERY_VALUE)?;
            let raw = read_raw_value(key.0, Some("Environment"))?;
            let values = raw
                .map(|(kind, bytes)| {
                    if kind != REG_MULTI_SZ {
                        return Err("service Environment is not REG_MULTI_SZ".into());
                    }
                    decode_multi_string(&bytes)
                })
                .transpose()?
                .unwrap_or_default();
            let display = if let Some(name) = extra.first() {
                values
                    .iter()
                    .find_map(|entry| {
                        entry
                            .split_once('=')
                            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                            .map(|(_, value)| value.to_owned())
                    })
                    .unwrap_or_default()
            } else {
                values.join("\r\n")
            };
            println!("{display}");
            Ok(())
        }
        "name" => {
            println!("{service}");
            Ok(())
        }
        "dependongroup" | "dependonservice" | "description" | "displayname" | "imagepath"
        | "objectname" | "start" | "type" => {
            let handle = open_service(service, SERVICE_QUERY_CONFIG)?;
            let (storage, config) = query_service_config(handle.0)?;
            let _keep_storage_alive = storage;
            let value = match parameter.to_ascii_lowercase().as_str() {
                "dependongroup" | "dependonservice" => {
                    let dependencies = unsafe { dependency_list(config.lpDependencies) };
                    let groups = parameter.eq_ignore_ascii_case("DependOnGroup");
                    let selected = dependencies
                        .into_iter()
                        .filter(|item| item.starts_with("+") == groups)
                        .collect::<Vec<_>>();
                    selected.join("\r\n")
                }
                "description" => service_description(handle.0)?,
                "displayname" => wide_pointer(config.lpDisplayName),
                "imagepath" => wide_pointer(config.lpBinaryPathName),
                "objectname" => wide_pointer(config.lpServiceStartName),
                "start" => {
                    let delayed = if config.dwStartType == SERVICE_AUTO_START {
                        let mut needed = 0;
                        let mut info = SERVICE_DELAYED_AUTO_START_INFO::default();
                        unsafe {
                            QueryServiceConfig2W(
                                handle.0,
                                SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                                (&mut info as *mut SERVICE_DELAYED_AUTO_START_INFO).cast(),
                                std::mem::size_of_val(&info) as u32,
                                &mut needed,
                            );
                        }
                        info.fDelayedAutostart != 0
                    } else {
                        false
                    };
                    if delayed {
                        "SERVICE_DELAYED_AUTO_START".into()
                    } else {
                        match config.dwStartType {
                            SERVICE_AUTO_START => "SERVICE_AUTO_START".into(),
                            SERVICE_DEMAND_START => "SERVICE_DEMAND_START".into(),
                            SERVICE_DISABLED => "SERVICE_DISABLED".into(),
                            _ => {
                                return Err(format!(
                                    "unsupported service start type: {}",
                                    config.dwStartType
                                ));
                            }
                        }
                    }
                }
                "type" => match config.dwServiceType {
                    SERVICE_WIN32_OWN_PROCESS => "SERVICE_WIN32_OWN_PROCESS".into(),
                    value if value == (SERVICE_WIN32_OWN_PROCESS | 0x100) => {
                        "SERVICE_INTERACTIVE_PROCESS".into()
                    }
                    _ => format!("SERVICE_TYPE_{}", config.dwServiceType),
                },
                _ => unreachable!(),
            };
            println!("{value}");
            Ok(())
        }
        _ => Err(format!("unsupported setting: {parameter}")),
    }
}

fn set_native_setting(service: &str, parameter: &str, values: &[String]) -> Result<(), String> {
    match parameter.to_ascii_lowercase().as_str() {
        "environment" => {
            let key = open_service_registry(service, true, KEY_QUERY_VALUE | KEY_SET_VALUE)?;
            let existing = read_raw_value(key.0, Some("Environment"))?
                .map(|(kind, bytes)| {
                    if kind != REG_MULTI_SZ {
                        return Err("service Environment is not REG_MULTI_SZ".into());
                    }
                    decode_multi_string(&bytes)
                })
                .transpose()?
                .unwrap_or_default();
            if values.is_empty() {
                return delete_raw_value(key.0, Some("Environment"));
            }
            let changed = edit_environment(&existing, values).map_err(|error| error.to_string())?;
            write_raw_value(
                key.0,
                Some("Environment"),
                REG_MULTI_SZ,
                &encode_multi_string(&changed),
            )
        }
        "name" => Err("the service Name setting cannot be changed".into()),
        "dependongroup" | "dependonservice" | "description" | "displayname" | "imagepath"
        | "objectname" | "start" | "type" => {
            let handle = open_service(service, SERVICE_CHANGE_CONFIG | SERVICE_QUERY_CONFIG)?;
            let value = values.join(" ");
            if parameter.eq_ignore_ascii_case("ImagePath") && value.is_empty() {
                return Err("ImagePath has no reset value".into());
            }
            if parameter.eq_ignore_ascii_case("Start") && value.is_empty() {
                return Err("Start has no reset value".into());
            }
            if parameter.eq_ignore_ascii_case("Type") && value.is_empty() {
                return Err("Type has no reset value".into());
            }
            let (_, config) = query_service_config(handle.0)?;
            let changed = match parameter.to_ascii_lowercase().as_str() {
                "dependongroup" | "dependonservice" => {
                    let dependencies = unsafe { dependency_list(config.lpDependencies) };
                    let groups = parameter.eq_ignore_ascii_case("DependOnGroup");
                    let mut services = dependencies
                        .iter()
                        .filter(|item| !item.starts_with("+"))
                        .cloned()
                        .collect::<Vec<_>>();
                    let mut group_names = dependencies
                        .iter()
                        .filter(|item| item.starts_with("+"))
                        .cloned()
                        .collect::<Vec<_>>();
                    if groups {
                        group_names = edit_dependencies(&group_names, values, true)
                            .map_err(|error| error.to_string())?;
                    } else {
                        services = edit_dependencies(&services, values, false)
                            .map_err(|error| error.to_string())?;
                    }
                    let block = dependency_block(&services, &group_names);
                    (unsafe {
                        ChangeServiceConfigW(
                            handle.0,
                            SERVICE_NO_CHANGE,
                            SERVICE_NO_CHANGE,
                            SERVICE_NO_CHANGE,
                            ptr::null(),
                            ptr::null(),
                            ptr::null_mut(),
                            block.as_ptr(),
                            ptr::null(),
                            ptr::null(),
                            ptr::null(),
                        )
                    }) != 0
                }
                "description" => {
                    let mut description = wide(OsStr::new(&value));
                    let info = SERVICE_DESCRIPTIONW {
                        lpDescription: description.as_mut_ptr(),
                    };
                    (unsafe {
                        ChangeServiceConfig2W(
                            handle.0,
                            SERVICE_CONFIG_DESCRIPTION,
                            (&info as *const SERVICE_DESCRIPTIONW).cast(),
                        )
                    }) != 0
                }
                "displayname" => {
                    let display = if values.is_empty() {
                        service.to_owned()
                    } else {
                        value
                    };
                    let display = wide(OsStr::new(&display));
                    (unsafe {
                        ChangeServiceConfigW(
                            handle.0,
                            SERVICE_NO_CHANGE,
                            SERVICE_NO_CHANGE,
                            SERVICE_NO_CHANGE,
                            ptr::null(),
                            ptr::null(),
                            ptr::null_mut(),
                            ptr::null(),
                            ptr::null(),
                            ptr::null(),
                            display.as_ptr(),
                        )
                    }) != 0
                }
                "imagepath" => {
                    let binary = wide(OsStr::new(&value));
                    (unsafe {
                        ChangeServiceConfigW(
                            handle.0,
                            SERVICE_NO_CHANGE,
                            SERVICE_NO_CHANGE,
                            SERVICE_NO_CHANGE,
                            binary.as_ptr(),
                            ptr::null(),
                            ptr::null_mut(),
                            ptr::null(),
                            ptr::null(),
                            ptr::null(),
                            ptr::null(),
                        )
                    }) != 0
                }
                "objectname" => {
                    let username = values.first().map(String::as_str).unwrap_or("LocalSystem");
                    let account = match username.to_ascii_lowercase().as_str() {
                        "localsystem" | "system" | "nt authority\\system" => "LocalSystem",
                        "localservice" | "local service" | "nt authority\\localservice" => {
                            "NT AUTHORITY\\LocalService"
                        }
                        "networkservice" | "network service" | "nt authority\\networkservice" => {
                            "NT AUTHORITY\\NetworkService"
                        }
                        _ => username,
                    };
                    let password_buffer =
                        if !nssm_r::object_account_requires_password(account, service) {
                            None
                        } else {
                            if values.len() < 2 {
                                return Err(
                                    "ObjectName requires a password for this account".into()
                                );
                            }
                            Some(wide(OsStr::new(&values[1..].join(" "))))
                        };
                    let password = password_buffer
                        .as_ref()
                        .map_or(ptr::null(), |buffer| buffer.as_ptr());
                    let account = wide(OsStr::new(account));
                    (unsafe {
                        ChangeServiceConfigW(
                            handle.0,
                            SERVICE_NO_CHANGE,
                            SERVICE_NO_CHANGE,
                            SERVICE_NO_CHANGE,
                            ptr::null(),
                            ptr::null(),
                            ptr::null_mut(),
                            ptr::null(),
                            account.as_ptr(),
                            password,
                            ptr::null(),
                        )
                    }) != 0
                }
                "start" => {
                    let ty = match value.to_ascii_uppercase().as_str() {
                        "SERVICE_AUTO_START" => SERVICE_AUTO_START,
                        "SERVICE_DELAYED_AUTO_START" => SERVICE_AUTO_START,
                        "SERVICE_DEMAND_START" => SERVICE_DEMAND_START,
                        "SERVICE_DISABLED" => SERVICE_DISABLED,
                        _ => return Err(format!("invalid service startup type: {value}")),
                    };
                    let changed = unsafe {
                        ChangeServiceConfigW(
                            handle.0,
                            SERVICE_NO_CHANGE,
                            ty,
                            SERVICE_NO_CHANGE,
                            ptr::null(),
                            ptr::null(),
                            ptr::null_mut(),
                            ptr::null(),
                            ptr::null(),
                            ptr::null(),
                            ptr::null(),
                        )
                    } != 0;
                    if !changed {
                        false
                    } else {
                        let delayed = SERVICE_DELAYED_AUTO_START_INFO {
                            fDelayedAutostart: i32::from(
                                value.eq_ignore_ascii_case("SERVICE_DELAYED_AUTO_START"),
                            ),
                        };
                        (unsafe {
                            ChangeServiceConfig2W(
                                handle.0,
                                SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                                (&delayed as *const SERVICE_DELAYED_AUTO_START_INFO).cast(),
                            )
                        }) != 0
                            || io::Error::last_os_error().raw_os_error() == Some(124)
                    }
                }
                "type" => {
                    let service_type = match value.to_ascii_uppercase().as_str() {
                        "SERVICE_WIN32_OWN_PROCESS" => SERVICE_WIN32_OWN_PROCESS,
                        "SERVICE_INTERACTIVE_PROCESS" => {
                            if !wide_pointer(config.lpServiceStartName)
                                .eq_ignore_ascii_case("LocalSystem")
                                && !wide_pointer(config.lpServiceStartName)
                                    .eq_ignore_ascii_case("NT AUTHORITY\\SYSTEM")
                            {
                                return Err("interactive services must run as LocalSystem".into());
                            }
                            SERVICE_WIN32_OWN_PROCESS | 0x100
                        }
                        _ => return Err(format!("invalid service type: {value}")),
                    };
                    (unsafe {
                        ChangeServiceConfigW(
                            handle.0,
                            service_type,
                            SERVICE_NO_CHANGE,
                            SERVICE_NO_CHANGE,
                            ptr::null(),
                            ptr::null(),
                            ptr::null_mut(),
                            ptr::null(),
                            ptr::null(),
                            ptr::null(),
                            ptr::null(),
                        )
                    }) != 0
                }
                _ => unreachable!(),
            };
            if changed {
                Ok(())
            } else {
                Err(io::Error::last_os_error().to_string())
            }
        }
        _ => Err(format!("unsupported setting: {parameter}")),
    }
}

fn reset_native_setting(service: &str, parameter: &str, extra: &[String]) -> Result<(), String> {
    match parameter.to_ascii_lowercase().as_str() {
        "description" => set_native_setting(service, parameter, &[]),
        "displayname" => set_native_setting(service, parameter, &[]),
        "objectname" => set_native_setting(service, parameter, &["LocalSystem".into()]),
        "environment" => set_native_setting(service, parameter, &[]),
        "dependongroup" | "dependonservice" => set_native_setting(service, parameter, &[]),
        "imagepath" | "start" | "type" | "name" => Err(format!("{parameter} has no reset value")),
        _ => {
            let _ = extra;
            Err(format!("unsupported setting: {parameter}"))
        }
    }
}

fn dump_set_line(
    executable: &str,
    service: &str,
    parameter: &str,
    additional: Option<&str>,
    value: &str,
) {
    dump_set_line_values(executable, service, parameter, additional, &[value]);
}

fn dump_set_line_values(
    executable: &str,
    service: &str,
    parameter: &str,
    additional: Option<&str>,
    values: &[&str],
) {
    let mut arguments = vec![
        quote_windows_arg(executable),
        "set".into(),
        quote_windows_arg(service),
        quote_windows_arg(parameter),
    ];
    if let Some(additional) = additional {
        arguments.push(quote_windows_arg(additional));
    }
    arguments.extend(values.iter().map(|value| quote_windows_arg(value)));
    println!("{}", arguments.join(" "));
}

fn registry_names(key: HKEY, subkeys: bool) -> Result<Vec<String>, String> {
    let mut index = 0;
    let mut names = Vec::new();
    loop {
        let mut name = vec![0u16; 32_768];
        let mut name_len = name.len() as u32;
        let code = unsafe {
            if subkeys {
                RegEnumKeyExW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            } else {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut name_len,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            }
        };
        if code == ERROR_NO_MORE_ITEMS {
            break;
        }
        if code != 0 {
            return Err(registry_error(code));
        }
        name.truncate(name_len as usize);
        names.push(String::from_utf16_lossy(&name));
        index += 1;
    }
    Ok(names)
}

fn dump_managed_settings(executable: &str, source: &str, target: &str) -> Result<(), String> {
    for spec in MANAGED_SETTINGS {
        match spec.kind {
            SettingKind::String => {
                if spec.name.eq_ignore_ascii_case("Application") {
                    continue;
                }
                let (key, name) = setting_location(source, spec, None, false)?;
                if let Some((kind, bytes)) = read_raw_value(key.0, name.as_deref())? {
                    let value = if kind == REG_DWORD {
                        return Err(format!("unexpected DWORD value for {}", spec.name));
                    } else {
                        decode_wide(&bytes)?
                    };
                    if !value.is_empty() {
                        dump_set_line(executable, target, spec.name, None, &value);
                    }
                }
            }
            SettingKind::Dword => {
                let (key, name) = setting_location(source, spec, None, false)?;
                if let Some((kind, bytes)) = read_raw_value(key.0, name.as_deref())? {
                    if kind != REG_DWORD || bytes.len() != 4 {
                        return Err(format!("invalid registry value for {}", spec.name));
                    }
                    let value = u32::from_le_bytes(
                        bytes.try_into().map_err(|_| "invalid DWORD".to_owned())?,
                    );
                    dump_set_line(executable, target, spec.name, None, &value.to_string());
                }
            }
            SettingKind::MultiString => {
                let (key, name) = setting_location(source, spec, None, false)?;
                if let Some((kind, bytes)) = read_raw_value(key.0, name.as_deref())? {
                    if kind != REG_MULTI_SZ {
                        return Err(format!("invalid registry value for {}", spec.name));
                    }
                    for (index, value) in decode_multi_string(&bytes)?.iter().enumerate() {
                        let edit = format!("{}{value}", if index == 0 { ':' } else { '+' });
                        dump_set_line(executable, target, spec.name, None, &edit);
                    }
                }
            }
            SettingKind::ExitAction => {
                let Some(key) = open_registry_key_optional(source, "AppExit", KEY_QUERY_VALUE)?
                else {
                    continue;
                };
                for code in registry_names(key.0, false)? {
                    let value_name = if code.is_empty() {
                        None
                    } else {
                        Some(code.as_str())
                    };
                    if let Some((_, bytes)) = read_raw_value(key.0, value_name)? {
                        let value = decode_wide(&bytes)?;
                        let additional = if code.is_empty() { "Default" } else { &code };
                        dump_set_line(executable, target, spec.name, Some(additional), &value);
                    }
                }
            }
            SettingKind::Hook => {
                let Some(root) = open_registry_key_optional(
                    source,
                    "AppEvents",
                    KEY_QUERY_VALUE | KEY_ENUMERATE_SUB_KEYS,
                )?
                else {
                    continue;
                };
                for event in registry_names(root.0, true)? {
                    let Some(key) = open_registry_key_optional(
                        source,
                        &format!("AppEvents\\{event}"),
                        KEY_QUERY_VALUE,
                    )?
                    else {
                        continue;
                    };
                    for action in registry_names(key.0, false)? {
                        if !valid_hook(&event, &action) {
                            continue;
                        }
                        if let Some((_, bytes)) = read_raw_value(key.0, Some(&action))? {
                            let value = decode_wide(&bytes)?;
                            if !value.is_empty() {
                                dump_set_line(
                                    executable,
                                    target,
                                    spec.name,
                                    Some(&format!("{event}/{action}")),
                                    &value,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn dump_native_settings(executable: &str, source: &str, target: &str) -> Result<(), String> {
    let service = open_service(source, SERVICE_QUERY_CONFIG)?;
    let (storage, config) = query_service_config(service.0)?;
    let _storage_lifetime = storage;
    let description = service_description(service.0)?;
    if !description.is_empty() {
        dump_set_line(executable, target, "Description", None, &description);
    }
    let display_name = wide_pointer(config.lpDisplayName);
    if !display_name.is_empty() && !display_name.eq_ignore_ascii_case(source) {
        dump_set_line(executable, target, "DisplayName", None, &display_name);
    }
    let account = wide_pointer(config.lpServiceStartName);
    if !account.eq_ignore_ascii_case("LocalSystem")
        && !account.eq_ignore_ascii_case("NT AUTHORITY\\SYSTEM")
    {
        let password = if account.eq_ignore_ascii_case("NT AUTHORITY\\LocalService")
            || account.eq_ignore_ascii_case("NT AUTHORITY\\NetworkService")
            || account.ends_with('$')
        {
            None
        } else {
            Some("****")
        };
        if let Some(password) = password {
            dump_set_line_values(
                executable,
                target,
                "ObjectName",
                None,
                &[&account, password],
            );
        } else {
            dump_set_line(executable, target, "ObjectName", None, &account);
        }
    }
    let startup = match config.dwStartType {
        SERVICE_AUTO_START => {
            let mut info = SERVICE_DELAYED_AUTO_START_INFO::default();
            let mut needed = 0;
            let delayed = unsafe {
                QueryServiceConfig2W(
                    service.0,
                    SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                    (&mut info as *mut SERVICE_DELAYED_AUTO_START_INFO).cast(),
                    std::mem::size_of_val(&info) as u32,
                    &mut needed,
                )
            } != 0
                && info.fDelayedAutostart != 0;
            if delayed {
                "SERVICE_DELAYED_AUTO_START"
            } else {
                "SERVICE_AUTO_START"
            }
        }
        SERVICE_DEMAND_START => "SERVICE_DEMAND_START",
        SERVICE_DISABLED => "SERVICE_DISABLED",
        _ => {
            return Err(format!(
                "unsupported service startup type: {}",
                config.dwStartType
            ));
        }
    };
    dump_set_line(executable, target, "Start", None, startup);
    let service_type = match config.dwServiceType {
        SERVICE_WIN32_OWN_PROCESS => "SERVICE_WIN32_OWN_PROCESS",
        value if value == (SERVICE_WIN32_OWN_PROCESS | 0x100) => "SERVICE_INTERACTIVE_PROCESS",
        value => return Err(format!("unsupported service type: {value}")),
    };
    dump_set_line(executable, target, "Type", None, service_type);

    let dependencies = unsafe { dependency_list(config.lpDependencies) };
    for groups in [false, true] {
        let values = dependencies
            .iter()
            .filter(|value| value.starts_with('+') == groups)
            .collect::<Vec<_>>();
        for (index, dependency) in values.iter().enumerate() {
            let edit = if groups {
                if index == 0 {
                    format!(":{dependency}")
                } else {
                    format!("+{dependency}")
                }
            } else if index == 0 {
                format!(":{dependency}")
            } else {
                format!("+{dependency}")
            };
            dump_set_line(
                executable,
                target,
                if groups {
                    "DependOnGroup"
                } else {
                    "DependOnService"
                },
                None,
                &edit,
            );
        }
    }
    let root = open_service_registry(source, false, KEY_QUERY_VALUE)?;
    if let Some((kind, bytes)) = read_raw_value(root.0, Some("Environment"))? {
        if kind != REG_MULTI_SZ {
            return Err("service Environment is not REG_MULTI_SZ".into());
        }
        for (index, value) in decode_multi_string(&bytes)?.iter().enumerate() {
            let edit = format!("{}{value}", if index == 0 { ':' } else { '+' });
            dump_set_line(executable, target, "Environment", None, &edit);
        }
    }
    Ok(())
}

fn dump_service(source: &str, target: Option<&str>) -> Result<(), String> {
    let application = read_optional_reg_string(source, "Application")?
        .ok_or_else(|| format!("service {source} is not managed by nssm-r"))?;
    let target = target.unwrap_or(source);
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let executable = executable.to_string_lossy();
    let install = vec![
        quote_windows_arg(&executable),
        "install".into(),
        quote_windows_arg(target),
        quote_windows_arg(&application),
    ];
    println!("{}", install.join(" "));
    dump_managed_settings(&executable, source, target)?;
    dump_native_settings(&executable, source, target)
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        if self.job != 0 {
            unsafe {
                CloseHandle(self.job as HANDLE);
            }
        }
    }
}

fn assign_to_kill_on_close_job(
    mut child: Child,
    kill_process_tree: bool,
) -> Result<ManagedChild, String> {
    if !kill_process_tree {
        return Ok(ManagedChild { child, job: 0 });
    }
    let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if job.is_null() {
        let _ = child.kill();
        return Err(io::Error::last_os_error().to_string());
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let set = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of_val(&limits) as u32,
        )
    };
    if set == 0 || unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) } == 0 {
        let error = io::Error::last_os_error().to_string();
        let _ = child.kill();
        unsafe {
            CloseHandle(job);
        }
        return Err(error);
    }
    Ok(ManagedChild {
        child,
        job: job as isize,
    })
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn ensure_event_source() -> Result<(), String> {
    let registry_path = wide(OsStr::new(&format!(
        "SYSTEM\\CurrentControlSet\\Services\\EventLog\\Application\\{EVENT_SOURCE}"
    )));
    let mut key = ptr::null_mut();
    let code = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            registry_path.as_ptr(),
            0,
            ptr::null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if code != 0 {
        return Err(registry_error(code));
    }

    let result = (|| {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let message_file = wide(executable.as_os_str());
        let message_file_name = wide(OsStr::new("EventMessageFile"));
        let code = unsafe {
            RegSetValueExW(
                key,
                message_file_name.as_ptr(),
                0,
                REG_SZ,
                message_file.as_ptr().cast(),
                (message_file.len() * std::mem::size_of::<u16>()) as u32,
            )
        };
        if code != 0 {
            return Err(registry_error(code));
        }
        let types_name = wide(OsStr::new("TypesSupported"));
        let types_supported = u32::from(EVENTLOG_ERROR_TYPE)
            | u32::from(windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE)
            | u32::from(windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE);
        let code = unsafe {
            RegSetValueExW(
                key,
                types_name.as_ptr(),
                0,
                REG_DWORD,
                (&types_supported as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        };
        if code != 0 {
            return Err(registry_error(code));
        }
        Ok(())
    })();
    unsafe { RegCloseKey(key) };
    result
}

fn report_event(event_type: u16, event_id: u32, values: &[&str]) {
    let source = wide(OsStr::new(EVENT_SOURCE));
    let handle = unsafe { RegisterEventSourceW(ptr::null(), source.as_ptr()) };
    if handle.is_null() {
        return;
    }
    let strings = values
        .iter()
        .map(|value| {
            value
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let string_pointers = strings
        .iter()
        .map(|value| value.as_ptr())
        .collect::<Vec<_>>();
    unsafe {
        ReportEventW(
            handle,
            event_type,
            0,
            event_id,
            ptr::null_mut(),
            string_pointers.len().min(u16::MAX as usize) as u16,
            0,
            string_pointers.as_ptr(),
            ptr::null(),
        );
        DeregisterEventSource(handle);
    }
}

struct ScHandle(SC_HANDLE);
impl Drop for ScHandle {
    fn drop(&mut self) {
        unsafe { CloseServiceHandle(self.0) };
    }
}

fn open_manager(access: u32) -> Result<ScHandle, String> {
    let h = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), access) };
    if h.is_null() {
        Err(io::Error::last_os_error().to_string())
    } else {
        Ok(ScHandle(h))
    }
}

fn open_service(name: &str, access: u32) -> Result<ScHandle, String> {
    let manager = open_manager(SC_MANAGER_CONNECT)?;
    let name = wide(OsStr::new(name));
    let h = unsafe { OpenServiceW(manager.0, name.as_ptr(), access) };
    if h.is_null() {
        Err(io::Error::last_os_error().to_string())
    } else {
        Ok(ScHandle(h))
    }
}

fn state(service: SC_HANDLE) -> Result<u32, String> {
    let mut status = SERVICE_STATUS::default();
    if unsafe { QueryServiceStatus(service, &mut status) } == 0 {
        Err(io::Error::last_os_error().to_string())
    } else {
        Ok(status.dwCurrentState)
    }
}

fn state_name(code: u32) -> &'static str {
    match code {
        SERVICE_STOPPED => "SERVICE_STOPPED",
        SERVICE_START_PENDING => "SERVICE_START_PENDING",
        SERVICE_STOP_PENDING => "SERVICE_STOP_PENDING",
        SERVICE_RUNNING => "SERVICE_RUNNING",
        SERVICE_CONTINUE_PENDING => "SERVICE_CONTINUE_PENDING",
        SERVICE_PAUSE_PENDING => "SERVICE_PAUSE_PENDING",
        SERVICE_PAUSED => "SERVICE_PAUSED",
        _ => "SERVICE_UNKNOWN",
    }
}

fn start(name: &str) -> Result<(), String> {
    let service = open_service(name, SERVICE_START | SERVICE_QUERY_STATUS)?;
    if state(service.0)? == SERVICE_RUNNING {
        return Ok(());
    }
    if unsafe { StartServiceW(service.0, 0, ptr::null()) } == 0 {
        let error = io::Error::last_os_error();
        // ERROR_SERVICE_ALREADY_RUNNING may race with another controller.
        if error.raw_os_error() != Some(1056) {
            return Err(error.to_string());
        }
    }
    wait_state(service.0, SERVICE_RUNNING)
}

fn stop(name: &str) -> Result<(), String> {
    let service = open_service(name, SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    let current = state(service.0)?;
    if current == SERVICE_STOPPED {
        return Ok(());
    }
    let mut status = SERVICE_STATUS::default();
    if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(1062) {
            return Err(error.to_string());
        }
    }
    wait_state(service.0, SERVICE_STOPPED)
}

fn control_simple(name: &str, control: u32) -> Result<(), String> {
    let service = open_service(name, SERVICE_PAUSE_CONTINUE | SERVICE_QUERY_STATUS)?;
    let mut status = SERVICE_STATUS::default();
    if unsafe { ControlService(service.0, control, &mut status) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    wait_state(
        service.0,
        if control == SERVICE_CONTROL_PAUSE {
            SERVICE_PAUSED
        } else {
            SERVICE_RUNNING
        },
    )
}

fn rotate(name: &str) -> Result<(), String> {
    let service = open_service(name, SERVICE_USER_DEFINED_CONTROL)?;
    let mut status = SERVICE_STATUS::default();
    if unsafe { ControlService(service.0, 128, &mut status) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn wait_state(service: SC_HANDLE, expected: u32) -> Result<(), String> {
    for _ in 0..300 {
        let current = state(service)?;
        if current == expected {
            return Ok(());
        }
        if current == SERVICE_STOPPED {
            return Err(format!(
                "service stopped before reaching {}",
                state_name(expected)
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!("timed out waiting for {}", state_name(expected)))
}

fn remove(name: &str, confirmed: bool) -> Result<(), String> {
    if !confirmed {
        eprint!("Remove service '{name}'? [y/N] ");
        let mut response = String::new();
        io::stdin()
            .read_line(&mut response)
            .map_err(|e| e.to_string())?;
        if !matches!(response.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return Err("removal cancelled".into());
        }
    }
    let service = open_service(name, DELETE_ACCESS | SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    if state(service.0)? != SERVICE_STOPPED {
        stop(name)?;
    }
    if unsafe { DeleteService(service.0) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn list_services(include_native: bool) -> Result<(), String> {
    let manager = open_manager(SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE)?;
    let mut required = 0;
    let mut returned = 0;
    let mut resume = 0;
    unsafe {
        EnumServicesStatusExW(
            manager.0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            ptr::null_mut(),
            0,
            &mut required,
            &mut returned,
            &mut resume,
            ptr::null(),
        );
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(234) {
        return Err(error.to_string());
    }
    let mut storage = vec![0usize; (required as usize).div_ceil(std::mem::size_of::<usize>())];
    loop {
        let mut bytes_needed = 0;
        let mut count = 0;
        let ok = unsafe {
            EnumServicesStatusExW(
                manager.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                storage.as_mut_ptr().cast(),
                (storage.len() * std::mem::size_of::<usize>()) as u32,
                &mut bytes_needed,
                &mut count,
                &mut resume,
                ptr::null(),
            )
        };
        if ok == 0 && io::Error::last_os_error().raw_os_error() != Some(234) {
            return Err(io::Error::last_os_error().to_string());
        }
        let entries = storage.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>();
        for index in 0..count as usize {
            let entry = unsafe { &*entries.add(index) };
            let mut len = 0;
            while unsafe { *entry.lpServiceName.add(len) } != 0 {
                len += 1;
            }
            let name = String::from_utf16_lossy(unsafe {
                std::slice::from_raw_parts(entry.lpServiceName, len)
            });
            let managed = read_optional_reg_string(&name, "Application")
                .ok()
                .flatten()
                .is_some();
            if include_native || managed {
                println!("{name}");
            }
        }
        if ok != 0 {
            break;
        }
        if bytes_needed as usize <= storage.len() * std::mem::size_of::<usize>() {
            return Err("SCM service enumeration made no progress".into());
        }
        storage.resize(
            (bytes_needed as usize).div_ceil(std::mem::size_of::<usize>()),
            0,
        );
    }
    Ok(())
}

#[derive(Clone)]
struct ProcessEntry {
    pid: u32,
    name: String,
}

fn process_image(pid: u32, fallback: &str) -> String {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return fallback.to_owned();
    }
    let mut path = vec![0u16; 32768];
    let mut size = path.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut size)
    };
    unsafe {
        CloseHandle(process);
    }
    if ok == 0 {
        fallback.to_owned()
    } else {
        String::from_utf16_lossy(&path[..size as usize])
    }
}

fn process_tree(service: &str) -> Result<(), String> {
    let service_handle = open_service(service, SERVICE_QUERY_STATUS)?;
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0;
    if unsafe {
        QueryServiceStatusEx(
            service_handle.0,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            std::mem::size_of_val(&status) as u32,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().to_string());
    }
    let root = status.dwProcessId;
    if root == 0 {
        return Ok(());
    }
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
        return Err(io::Error::last_os_error().to_string());
    }
    let mut processes = HashMap::<u32, Vec<ProcessEntry>>::new();
    let mut entry = PROCESSENTRY32W::default();
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        let length = entry
            .szExeFile
            .iter()
            .position(|ch| *ch == 0)
            .unwrap_or(entry.szExeFile.len());
        processes
            .entry(entry.th32ParentProcessID)
            .or_default()
            .push(ProcessEntry {
                pid: entry.th32ProcessID,
                name: String::from_utf16_lossy(&entry.szExeFile[..length]),
            });
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    unsafe {
        CloseHandle(snapshot);
    }
    fn visit(
        pid: u32,
        depth: usize,
        name: &str,
        tree: &HashMap<u32, Vec<ProcessEntry>>,
        seen: &mut HashSet<u32>,
    ) {
        if !seen.insert(pid) {
            return;
        }
        println!("{pid:>8} {}{}", " ".repeat(depth), process_image(pid, name));
        if let Some(children) = tree.get(&pid) {
            for child in children {
                visit(child.pid, depth + 1, &child.name, tree, seen);
            }
        }
    }
    visit(root, 0, "???", &processes, &mut HashSet::new());
    Ok(())
}

fn processes(services: &[String]) -> Result<(), String> {
    let mut errors = Vec::new();
    for service in services {
        if let Err(error) = process_tree(service) {
            errors.push(format!("{service}: {error}"));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn registry_error(code: u32) -> String {
    io::Error::from_raw_os_error(code as i32).to_string()
}

fn write_reg_string(key: HKEY, name: &str, value: &str) -> Result<(), String> {
    let value_name = wide(OsStr::new(name));
    let mut data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let code = unsafe {
        RegSetValueExW(
            key,
            value_name.as_ptr(),
            0,
            REG_EXPAND_SZ,
            data.as_mut_ptr().cast(),
            (data.len() * 2) as u32,
        )
    };
    if code == 0 {
        Ok(())
    } else {
        Err(registry_error(code))
    }
}

fn write_service_config(name: &str, app: &str, args: &[String]) -> Result<(), String> {
    let path = wide(OsStr::new(&format!(
        "{SERVICE_KEY_PREFIX}{name}\\Parameters"
    )));
    let mut key = ptr::null_mut();
    let code = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        )
    };
    if code != 0 {
        return Err(registry_error(code));
    }
    let result = write_reg_string(key, "Application", app).and_then(|_| {
        let joined = args
            .iter()
            .map(|s| quote_windows_arg(s))
            .collect::<Vec<_>>()
            .join(" ");
        write_reg_string(key, "AppParameters", &joined)
    });
    unsafe {
        RegCloseKey(key);
    }
    result
}

fn install(name: &str, application: Option<&str>, args: &[String]) -> Result<(), String> {
    let app = application.ok_or_else(|| {
        "install requires an application path (GUI installation is not supported)".to_owned()
    })?;
    if app.is_empty() {
        return Err("application path cannot be empty".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin_path = format!(
        "{} __service {}",
        quote_windows_arg(&exe.to_string_lossy()),
        quote_windows_arg(name)
    );
    let service_name = wide(OsStr::new(name));
    let binary_path = wide(OsStr::new(&bin_path));
    let manager = open_manager(SC_MANAGER_CREATE_SERVICE)?;
    let service = unsafe {
        CreateServiceW(
            manager.0,
            service_name.as_ptr(),
            service_name.as_ptr(),
            SERVICE_QUERY_STATUS | SERVICE_START | SERVICE_STOP | DELETE_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            binary_path.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
        )
    };
    if service.is_null() {
        return Err(io::Error::last_os_error().to_string());
    }
    let service = ScHandle(service);
    if let Err(error) = write_service_config(name, app, args) {
        unsafe {
            DeleteService(service.0);
        }
        return Err(error);
    }
    println!("Installed service '{name}'.");
    Ok(())
}

fn read_reg_string(service: &str, value: &str) -> Result<String, String> {
    read_optional_reg_string(service, value)?
        .ok_or_else(|| format!("missing service setting {value}"))
}

fn read_optional_reg_string(service: &str, value: &str) -> Result<Option<String>, String> {
    let path = wide(OsStr::new(&format!(
        "{SERVICE_KEY_PREFIX}{service}\\Parameters"
    )));
    let value_name = wide(OsStr::new(value));
    let mut key = ptr::null_mut();
    let code = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if code != 0 {
        return Err(registry_error(code));
    }
    let mut bytes = 0;
    let code = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if code == 2 {
        unsafe {
            RegCloseKey(key);
        }
        return Ok(None);
    }
    if code != 0 {
        unsafe {
            RegCloseKey(key);
        }
        return Err(registry_error(code));
    }
    let mut data = vec![0u16; (bytes as usize + 1) / 2];
    let code = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            data.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    unsafe {
        RegCloseKey(key);
    }
    if code != 0 {
        return Err(registry_error(code));
    }
    if let Some(end) = data.iter().position(|c| *c == 0) {
        data.truncate(end);
    }
    Ok(Some(String::from_utf16_lossy(&data)))
}

fn read_reg_dword(service: &str, value: &str, default: u32) -> Result<u32, String> {
    let path = wide(OsStr::new(&format!(
        "{SERVICE_KEY_PREFIX}{service}\\Parameters"
    )));
    let value_name = wide(OsStr::new(value));
    let mut key = ptr::null_mut();
    let code = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if code != 0 {
        return Err(registry_error(code));
    }
    let mut kind = 0;
    let mut data = 0u32;
    let mut bytes = std::mem::size_of::<u32>() as u32;
    let code = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            ptr::null(),
            &mut kind,
            (&mut data as *mut u32).cast(),
            &mut bytes,
        )
    };
    unsafe {
        RegCloseKey(key);
    }
    if code == 2 {
        return Ok(default);
    }
    if code != 0 {
        return Err(registry_error(code));
    }
    if kind != REG_DWORD || bytes != std::mem::size_of::<u32>() as u32 {
        return Err(format!("service setting {value} is not a DWORD"));
    }
    Ok(data)
}

fn open_stdio_file(
    service: &str,
    setting: &str,
    child_environment: &[String],
    input: bool,
) -> Result<Option<File>, String> {
    let Some(path) = read_optional_reg_string(service, setting)? else {
        return Ok(None);
    };
    let path = expand_environment_value(&path, child_environment);
    let prefix = format!("{setting}");
    let default_sharing = if input { 2 } else { 3 };
    let default_disposition = if input { 3 } else { 4 };
    let sharing = read_reg_dword(service, &format!("{prefix}ShareMode"), default_sharing)?;
    let disposition = read_reg_dword(
        service,
        &format!("{prefix}CreationDisposition"),
        default_disposition,
    )?;
    let attributes = read_reg_dword(service, &format!("{prefix}FlagsAndAttributes"), 128)?;
    let mut options = OpenOptions::new();
    options
        .read(input)
        .write(!input)
        .append(!input && disposition != 2 && disposition != 5)
        .share_mode(sharing)
        .attributes(attributes);
    match disposition {
        1 => {
            options.create_new(true);
        }
        2 => {
            options.create(true).truncate(true);
        }
        3 => {}
        4 => {
            options.create(true);
        }
        5 => {
            options.truncate(true);
        }
        _ => {
            return Err(format!(
                "invalid {setting} creation disposition: {disposition}"
            ));
        }
    }
    let mut file = options
        .open(&path)
        .map_err(|error| format!("unable to open {setting} file {path}: {error}"))?;
    if !input {
        file.seek(SeekFrom::End(0))
            .map_err(|error| format!("unable to seek to end of {setting} file {path}: {error}"))?;
    }
    Ok(Some(file))
}

fn open_hook_stdio_file(
    service: &str,
    setting: &str,
    child_environment: &[String],
) -> Result<Option<File>, String> {
    let Some(path) = read_optional_reg_string(service, setting)? else {
        return Ok(None);
    };
    let path = expand_environment_value(&path, child_environment);
    let sharing = read_reg_dword(service, &format!("{setting}ShareMode"), 3)?;
    let attributes = read_reg_dword(service, &format!("{setting}FlagsAndAttributes"), 128)?;
    let file = OpenOptions::new()
        .write(true)
        .append(true)
        .share_mode(sharing)
        .attributes(attributes)
        .open(&path)
        .map_err(|error| format!("unable to open hook {setting} file {path}: {error}"))?;
    Ok(Some(file))
}

fn utc_timestamp_prefix() -> String {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = elapsed.as_secs();
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
    year += i64::from(month <= 2);
    let seconds_of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03}: ",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60,
        elapsed.subsec_millis()
    )
}

fn log_stream<R: Read>(
    mut input: R,
    destination: Arc<LogDestination>,
    timestamp: bool,
) -> io::Result<()> {
    let mut buffer = [0; 4096];
    let mut line_start = true;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return destination.flush();
        }
        let mut chunk = Vec::with_capacity(count + count / 40 * 25);
        for byte in &buffer[..count] {
            if timestamp && line_start {
                chunk.extend_from_slice(utc_timestamp_prefix().as_bytes());
            }
            chunk.push(*byte);
            line_start = *byte == b'\n';
        }
        destination.write_all(&chunk)?;
    }
}

fn rotate_log_file(
    path: &str,
    minimum_age: Duration,
    minimum_size: u64,
    delay: Duration,
    force: bool,
    copy_and_truncate: bool,
    live_handle: bool,
) -> Result<bool, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    let modified = metadata.modified().map_err(|error| error.to_string())?;
    let age = modified.elapsed().unwrap_or_default();
    if !log_rotation_due(metadata.len(), age, minimum_age, minimum_size, force) {
        return Ok(false);
    }
    let rotated = rotated_log_path(Path::new(path), modified)
        .ok_or_else(|| format!("could not create rotated name for {path}"))?;
    if copy_and_truncate || live_handle {
        fs::copy(path, &rotated).map_err(|error| format!("unable to copy {path}: {error}"))?;
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|error| format!("unable to reopen {path}: {error}"))?;
        file.set_len(0)
            .map_err(|error| format!("unable to truncate {path}: {error}"))?;
    } else {
        fs::rename(path, &rotated).map_err(|error| format!("unable to rename {path}: {error}"))?;
    }
    Ok(true)
}

unsafe extern "system" fn post_close_to_process_window(hwnd: HWND, target_pid: LPARAM) -> i32 {
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
    if process_id == target_pid as u32 {
        let mut result = 0;
        unsafe {
            SendMessageTimeoutW(hwnd, WM_CLOSE, 0, 0, SMTO_ABORTIFHUNG, 1000, &mut result);
        }
    }
    1
}

fn send_console_ctrl_c(process_id: u32) {
    if unsafe { AttachConsole(process_id) } == 0 {
        return;
    }
    unsafe {
        SetConsoleCtrlHandler(None, 1);
        GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0);
        // Console control handlers run asynchronously. Keep the target console
        // attached briefly so the event can be dispatched before detaching.
        thread::sleep(Duration::from_millis(25));
        FreeConsole();
        SetConsoleCtrlHandler(None, 0);
    }
}

fn send_window_close(process_id: u32) {
    unsafe {
        EnumWindows(Some(post_close_to_process_window), process_id as LPARAM);
    }
}

fn send_thread_quit(process_id: u32) {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return;
    }
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while more {
        if entry.th32OwnerProcessID == process_id {
            unsafe { PostThreadMessageW(entry.th32ThreadID, WM_QUIT, 0 as WPARAM, 0 as LPARAM) };
        }
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
}

fn wait_process_for(child: &mut Child, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Err(_) => return true,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(None) => return false,
        }
    }
}

fn stop_child(service: &str, child: &mut ManagedChild, no_console: bool) -> Result<(), String> {
    let skip = read_reg_dword(service, "AppStopMethodSkip", 0)?;
    let pid = child.child.id();
    for method in nssm_r::stop_methods(skip, no_console) {
        match method {
            StopMethod::Console | StopMethod::Window | StopMethod::Threads => {
                let (setting, signal) = match method {
                    StopMethod::Console => ("AppStopMethodConsole", send_console_ctrl_c as fn(u32)),
                    StopMethod::Window => ("AppStopMethodWindow", send_window_close as fn(u32)),
                    StopMethod::Threads => ("AppStopMethodThreads", send_thread_quit as fn(u32)),
                    StopMethod::Terminate => unreachable!(),
                };
                signal(pid);
                let delay = read_reg_dword(service, setting, 1500)?;
                if wait_process_for(&mut child.child, Duration::from_millis(delay as u64)) {
                    return Ok(());
                }
            }
            StopMethod::Terminate => {
                if child.job != 0 {
                    unsafe { TerminateJobObject(child.job as HANDLE, 1) };
                }
                let _ = child.child.kill();
            }
        }
    }
    while child
        .child
        .try_wait()
        .map_err(|error| error.to_string())?
        .is_none()
    {
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn read_exit_rule(service: &str, exit_code: Option<u32>) -> Result<Option<ExitAction>, String> {
    let path = wide(OsStr::new(&format!(
        "{SERVICE_KEY_PREFIX}{service}\\Parameters\\AppExit"
    )));
    let mut key = ptr::null_mut();
    let code = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if code == 2 {
        return Ok(None);
    }
    if code != 0 {
        return Err(registry_error(code));
    }
    let value_name = exit_code.map(|code| wide(OsStr::new(&code.to_string())));
    let value_ptr = value_name
        .as_ref()
        .map_or(ptr::null(), |name| name.as_ptr());
    let mut bytes = 0;
    let code = unsafe {
        RegQueryValueExW(
            key,
            value_ptr,
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut bytes,
        )
    };
    if code == 2 {
        unsafe {
            RegCloseKey(key);
        }
        return Ok(None);
    }
    if code != 0 {
        unsafe {
            RegCloseKey(key);
        }
        return Err(registry_error(code));
    }
    let mut data = vec![0u16; (bytes as usize + 1) / 2];
    let code = unsafe {
        RegQueryValueExW(
            key,
            value_ptr,
            ptr::null(),
            ptr::null_mut(),
            data.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    unsafe {
        RegCloseKey(key);
    }
    if code != 0 {
        return Err(registry_error(code));
    }
    if let Some(end) = data.iter().position(|ch| *ch == 0) {
        data.truncate(end);
    }
    let text = String::from_utf16_lossy(&data);
    ExitAction::parse(&text)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn ensure_default_exit_action(service: &str) -> Result<(), String> {
    let key = open_registry_key(service, "AppExit", true, KEY_QUERY_VALUE | KEY_SET_VALUE)?;
    if read_raw_value(key.0, None)?.is_none() {
        write_raw_value(key.0, None, REG_SZ, &encode_wide("Restart", 1))?;
    }
    Ok(())
}

fn read_hook(service: &str, hook: &str) -> Result<Option<String>, String> {
    let (event, action) = hook
        .split_once('/')
        .ok_or_else(|| "hook must use event/action syntax".to_owned())?;
    if !valid_hook(event, action) {
        return Err("unsupported AppEvents event/action pair".into());
    }
    let path = wide(OsStr::new(&format!(
        "{SERVICE_KEY_PREFIX}{service}\\Parameters\\AppEvents\\{event}"
    )));
    let mut key = ptr::null_mut();
    let code = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        )
    };
    if code == 2 {
        return Ok(None);
    }
    if code != 0 {
        return Err(registry_error(code));
    }
    let result = read_raw_value(key, Some(action))?
        .map(|(_, data)| decode_wide(&data))
        .transpose();
    unsafe {
        RegCloseKey(key);
    }
    result
}

fn reap_async_hooks() {
    if let Ok(mut hooks) = ASYNC_HOOKS.lock() {
        hooks.retain_mut(|hook| match hook.try_wait() {
            Ok(Some(_)) => false,
            Ok(None) | Err(_) => true,
        });
    }
}

fn finish_async_hooks() {
    let Ok(mut slot) = ASYNC_HOOKS.lock() else {
        return;
    };
    let hooks = std::mem::take(&mut *slot);
    drop(slot);
    let deadline = Instant::now() + Duration::from_secs(30);
    for mut hook in hooks {
        loop {
            match hook.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                Ok(None) => {
                    let _ = hook.kill();
                    let _ = hook.wait();
                    break;
                }
            }
        }
    }
}

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u64::MAX as u128) as u64
        })
}

fn control_name(control: u32) -> &'static str {
    match control {
        0 => "START",
        SERVICE_CONTROL_STOP => "STOP",
        SERVICE_CONTROL_PAUSE => "PAUSE",
        SERVICE_CONTROL_CONTINUE => "CONTINUE",
        SERVICE_CONTROL_INTERROGATE => "INTERROGATE",
        SERVICE_CONTROL_SHUTDOWN => "SHUTDOWN",
        SERVICE_CONTROL_POWEREVENT => "POWEREVENT",
        128 => "ROTATE",
        _ => "",
    }
}

fn hook_deadline_ms(event: &str, action: &str) -> u64 {
    if (event.eq_ignore_ascii_case("Start") || event.eq_ignore_ascii_case("Stop"))
        && action.eq_ignore_ascii_case("Pre")
    {
        20_000
    } else {
        60_000
    }
}

fn invoke_hook(
    service: &str,
    event: &str,
    action: &str,
    child_environment: &[String],
    app_pid: Option<u32>,
    exit_code: Option<u32>,
    synchronous: bool,
    cancel_on_stop: bool,
) -> Result<Option<u32>, String> {
    let Some(command_line) = read_hook(service, &format!("{event}/{action}"))? else {
        return Ok(None);
    };
    if command_line.is_empty() {
        return Ok(None);
    }
    let args = split_windows_args(&command_line);
    let Some(program) = args.first() else {
        return Ok(None);
    };
    let mut command = ProcessCommand::new(program);
    command.args(args.iter().skip(1)).env_clear();
    for entry in child_environment {
        if let Some((key, value)) = entry.split_once('=') {
            command.env(key, value);
        }
    }
    let image = std::env::current_exe().map_err(|error| error.to_string())?;
    let application = read_optional_reg_string(service, "Application")?
        .map(|value| expand_environment_value(&value, child_environment))
        .unwrap_or_default();
    let working_directory = read_optional_reg_string(service, "AppDirectory")?
        .filter(|directory| !directory.is_empty())
        .map(|directory| expand_environment_value(&directory, child_environment))
        .or_else(|| {
            Path::new(&application)
                .parent()
                .filter(|directory| !directory.as_os_str().is_empty())
                .map(|directory| directory.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| {
            std::env::var("WINDIR")
                .map(|directory| format!("{directory}\\System32"))
                .unwrap_or_else(|_| r"C:\Windows\System32".to_owned())
        });
    command.current_dir(working_directory);
    let parameters = read_optional_reg_string(service, "AppParameters")?
        .map(|value| expand_environment_value(&value, child_environment))
        .unwrap_or_default();
    let application_command_line = format!("\"{application}\" {parameters}");
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
    let trigger = match event {
        "Start" => control_name(0),
        "Stop" | "Rotate" | "Power" => control_name(HOOK_LAST_CONTROL.load(Ordering::SeqCst)),
        _ => "",
    };
    let deadline_ms = hook_deadline_ms(event, action);
    let service_handle = open_service(service, SERVICE_QUERY_CONFIG)?;
    let (service_config_storage, service_config) = query_service_config(service_handle.0)?;
    let _keep_service_config_storage_alive = service_config_storage;
    let display_name = wide_pointer(service_config.lpDisplayName);
    let now = unix_time_millis();
    let service_start = HOOK_SERVICE_START_MS.load(Ordering::SeqCst);
    let app_start = HOOK_APPLICATION_START_MS.load(Ordering::SeqCst);
    let application_runtime = if app_pid.is_some() && app_start != 0 {
        now.saturating_sub(app_start)
    } else if event == "Start" && action == "Pre" {
        0
    } else {
        HOOK_APPLICATION_RUNTIME_MS.load(Ordering::SeqCst)
    };
    let application_runtime = if application_runtime == 0 && event == "Start" && action == "Pre" {
        String::new()
    } else {
        application_runtime.to_string()
    };
    let service_runtime = if service_start == 0 {
        String::new()
    } else {
        now.saturating_sub(service_start).to_string()
    };
    command
        .env("NSSM_HOOK_VERSION", "1")
        .env("NSSM_EVENT", event)
        .env("NSSM_ACTION", action)
        .env("NSSM_TRIGGER", trigger)
        .env("NSSM_CONFIGURATION", configuration)
        .env("NSSM_BUILD_DATE", env!("NSSM_BUILD_DATE"))
        .env("NSSM_SERVICE_NAME", service)
        .env("NSSM_SERVICE_DISPLAYNAME", display_name)
        .env("NSSM_EXE", image)
        .env("NSSM_VERSION", nssm_r::VERSION)
        .env("NSSM_PID", std::process::id().to_string())
        .env(
            "NSSM_LAST_CONTROL",
            control_name(HOOK_LAST_CONTROL.load(Ordering::SeqCst)),
        )
        .env(
            "NSSM_START_REQUESTED_COUNT",
            HOOK_START_REQUESTED_COUNT
                .load(Ordering::SeqCst)
                .to_string(),
        )
        .env(
            "NSSM_START_COUNT",
            HOOK_START_COUNT.load(Ordering::SeqCst).to_string(),
        )
        .env(
            "NSSM_THROTTLE_COUNT",
            HOOK_THROTTLE_COUNT.load(Ordering::SeqCst).to_string(),
        )
        .env(
            "NSSM_EXIT_COUNT",
            HOOK_EXIT_COUNT.load(Ordering::SeqCst).to_string(),
        )
        .env("NSSM_RUNTIME", service_runtime)
        .env("NSSM_APPLICATION_RUNTIME", application_runtime)
        .env(
            "NSSM_APPLICATION_PID",
            app_pid.map_or_else(String::new, |pid| pid.to_string()),
        )
        .env(
            "NSSM_EXITCODE",
            exit_code.map_or_else(String::new, |code| code.to_string()),
        )
        .env("NSSM_COMMAND_LINE", application_command_line)
        .env("NSSM_DEADLINE", deadline_ms.to_string());
    if read_reg_dword(service, "AppRedirectHook", 0)? != 0 {
        let stdout = open_hook_stdio_file(service, "AppStdout", child_environment)?;
        let stderr = open_hook_stdio_file(service, "AppStderr", child_environment)?;
        if let Some(stdout) = stdout {
            command.stdout(stdout);
        }
        if let Some(stderr) = stderr {
            command.stderr(stderr);
        }
    }
    let mut hook = command
        .spawn()
        .map_err(|error| format!("unable to launch {event}/{action} hook: {error}"))?;
    if !synchronous {
        reap_async_hooks();
        if let Ok(mut hooks) = ASYNC_HOOKS.lock() {
            hooks.push(hook);
        }
        return Ok(None);
    }
    let deadline = Instant::now() + Duration::from_millis(deadline_ms);
    loop {
        if let Some(status) = hook.try_wait().map_err(|error| error.to_string())? {
            return Ok(status.code().map(|code| code as u32));
        }
        if Instant::now() >= deadline || (cancel_on_stop && STOP_REQUESTED.load(Ordering::SeqCst)) {
            let _ = hook.kill();
            let _ = hook.wait();
            return Ok(Some(102));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn exit_action(service: &str, code: u32) -> Result<(ExitAction, bool), String> {
    if let Some(action) = read_exit_rule(service, Some(code))? {
        return Ok((action, false));
    }
    Ok((
        read_exit_rule(service, None)?.unwrap_or(ExitAction::Restart),
        true,
    ))
}

fn set_status(state: u32, exit_code: u32, wait_hint: u32) {
    set_status_with_service_exit(state, exit_code, 0, wait_hint);
}

fn set_status_with_service_exit(
    state: u32,
    win32_exit_code: u32,
    service_exit_code: u32,
    wait_hint: u32,
) {
    let handle = SERVICE_STATUS_HANDLE_VALUE.load(Ordering::SeqCst) as SERVICE_STATUS_HANDLE;
    if handle.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: match state {
            SERVICE_RUNNING | SERVICE_START_PENDING => {
                SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_POWEREVENT
            }
            SERVICE_PAUSED | SERVICE_CONTINUE_PENDING => {
                SERVICE_ACCEPT_STOP
                    | SERVICE_ACCEPT_SHUTDOWN
                    | SERVICE_ACCEPT_POWEREVENT
                    | SERVICE_ACCEPT_PAUSE_CONTINUE
            }
            _ => 0,
        },
        dwWin32ExitCode: win32_exit_code,
        dwServiceSpecificExitCode: service_exit_code,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    unsafe {
        SetServiceStatus(handle, &status);
    }
}

unsafe extern "system" fn service_handler(
    control: u32,
    event_type: u32,
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            HOOK_LAST_CONTROL.store(control, Ordering::SeqCst);
            STOP_REQUESTED.store(true, Ordering::SeqCst);
            set_status(SERVICE_STOP_PENDING, 0, 10_000);
        }
        128 => {
            if !ROTATE_ENABLED.load(Ordering::SeqCst) {
                return 120;
            }
            HOOK_LAST_CONTROL.store(control, Ordering::SeqCst);
            ROTATE_REQUESTED.store(true, Ordering::SeqCst);
        }
        SERVICE_CONTROL_POWEREVENT => {
            let Some(hook) = power_hook_action(event_type) else {
                return NO_ERROR;
            };
            HOOK_LAST_CONTROL.store(control, Ordering::SeqCst);
            POWER_HOOK_REQUESTED.store(hook, Ordering::SeqCst);
        }
        SERVICE_CONTROL_CONTINUE => {
            HOOK_LAST_CONTROL.store(control, Ordering::SeqCst);
            HOOK_THROTTLE_COUNT.store(0, Ordering::SeqCst);
            CONTINUE_REQUESTED.store(true, Ordering::SeqCst);
            set_status(SERVICE_CONTINUE_PENDING, 0, 10_000);
        }
        SERVICE_CONTROL_PAUSE => return 120,
        SERVICE_CONTROL_INTERROGATE => {}
        _ => return 120,
    }
    NO_ERROR
}

unsafe extern "system" fn service_main(argc: u32, argv: *mut windows_sys::core::PWSTR) {
    if argc == 0 || argv.is_null() {
        return;
    }
    let name_ptr = unsafe { *argv };
    if name_ptr.is_null() {
        return;
    }
    let mut len = 0;
    while unsafe { *name_ptr.add(len) } != 0 {
        len += 1;
    }
    let service_name =
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name_ptr, len) });
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    POWER_HOOK_REQUESTED.store(0, Ordering::SeqCst);
    CONTINUE_REQUESTED.store(false, Ordering::SeqCst);
    HOOK_START_REQUESTED_COUNT.store(0, Ordering::SeqCst);
    HOOK_START_COUNT.store(0, Ordering::SeqCst);
    HOOK_EXIT_COUNT.store(0, Ordering::SeqCst);
    HOOK_THROTTLE_COUNT.store(0, Ordering::SeqCst);
    HOOK_LAST_CONTROL.store(0, Ordering::SeqCst);
    HOOK_SERVICE_START_MS.store(unix_time_millis(), Ordering::SeqCst);
    HOOK_APPLICATION_START_MS.store(0, Ordering::SeqCst);
    HOOK_APPLICATION_RUNTIME_MS.store(0, Ordering::SeqCst);
    let name_wide = wide(OsStr::new(&service_name));
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(name_wide.as_ptr(), Some(service_handler), ptr::null())
    };
    if handle.is_null() {
        return;
    }
    SERVICE_STATUS_HANDLE_VALUE.store(handle as isize, Ordering::SeqCst);
    set_status(SERVICE_START_PENDING, 0, 10_000);
    let _ = ensure_default_exit_action(&service_name);
    let config = (|| -> Result<(String, Vec<String>, Option<String>, Option<Vec<String>>, Vec<String>, Vec<String>, Option<String>, u32, bool, bool), String> {
        let inherited: Vec<String> = std::env::vars_os().map(|(key, value)| format!("{}={}", key.to_string_lossy(), value.to_string_lossy())).collect();
        let app_environment = read_multi_parameter(&service_name, "AppEnvironment")?;
        let extra_environment = read_multi_parameter(&service_name, "AppEnvironmentExtra")?.unwrap_or_default();
        let child_environment = compose_child_environment(&inherited, app_environment.as_deref(), &extra_environment).map_err(|error| error.to_string())?;
        let app = expand_environment_value(&read_reg_string(&service_name, "Application")?, &child_environment);
        let args = read_optional_reg_string(&service_name, "AppParameters")?
            .map(|value| split_windows_args(&expand_environment_value(&value, &child_environment)))
            .unwrap_or_default();
        let directory = read_optional_reg_string(&service_name, "AppDirectory")?
            .map(|value| expand_environment_value(&value, &child_environment));
        let affinity = read_optional_reg_string(&service_name, "AppAffinity")?;
        let priority = read_reg_dword(&service_name, "AppPriority", 0x20)?;
        let no_console = read_reg_dword(&service_name, "AppNoConsole", 0)? != 0;
        let kill_process_tree = read_reg_dword(&service_name, "AppKillProcessTree", 1)? != 0;
        Ok((app, args, directory, app_environment, extra_environment, child_environment, affinity, priority, no_console, kill_process_tree))
    })();
    let (
        app,
        args,
        directory,
        app_environment,
        extra_environment,
        child_environment,
        affinity,
        priority,
        no_console,
        kill_process_tree,
    ) = match config {
        Ok(config) => config,
        Err(_) => {
            set_status(SERVICE_STOPPED, 2, 0);
            return;
        }
    };
    let throttle_ms = match read_reg_dword(&service_name, "AppThrottle", 1500) {
        Ok(value) => value,
        Err(_) => {
            set_status(SERVICE_STOPPED, 2, 0);
            return;
        }
    };
    let restart_delay_ms = match read_reg_dword(&service_name, "AppRestartDelay", 0) {
        Ok(value) => value,
        Err(_) => {
            set_status(SERVICE_STOPPED, 2, 0);
            return;
        }
    };
    let rotate_files = read_reg_dword(&service_name, "AppRotateFiles", 0).unwrap_or(0) != 0;
    let rotate_online = read_reg_dword(&service_name, "AppRotateOnline", 0).unwrap_or(0) != 0;
    ROTATE_ENABLED.store(rotate_online, Ordering::SeqCst);
    ROTATE_REQUESTED.store(false, Ordering::SeqCst);
    let rotate_seconds = read_reg_dword(&service_name, "AppRotateSeconds", 0).unwrap_or(0);
    let rotate_low = read_reg_dword(&service_name, "AppRotateBytes", 0).unwrap_or(0) as u64;
    let rotate_high = read_reg_dword(&service_name, "AppRotateBytesHigh", 0).unwrap_or(0) as u64;
    let rotate_size = (rotate_high << 32) | rotate_low;
    let rotate_delay = Duration::from_millis(
        read_reg_dword(&service_name, "AppRotateDelay", 0).unwrap_or(0) as u64,
    );
    let (stdout_path, stderr_path) = match (
        read_optional_reg_string(&service_name, "AppStdout"),
        read_optional_reg_string(&service_name, "AppStderr"),
    ) {
        (Ok(stdout), Ok(stderr)) => (
            stdout.map(|path| expand_environment_value(&path, &child_environment)),
            stderr.map(|path| expand_environment_value(&path, &child_environment)),
        ),
        _ => {
            set_status(SERVICE_STOPPED, 2, 0);
            return;
        }
    };
    let stdout_copy_and_truncate =
        read_reg_dword(&service_name, "AppStdoutCopyAndTruncate", 0).unwrap_or(0) != 0;
    let stderr_copy_and_truncate =
        read_reg_dword(&service_name, "AppStderrCopyAndTruncate", 0).unwrap_or(0) != 0;
    let timestamp_log = read_reg_dword(&service_name, "AppTimestampLog", 0).unwrap_or(0) != 0;
    let mut throttle = nssm_r::RestartThrottle::new(restart_delay_ms);
    let mut stop_hook_invoked = false;
    while !STOP_REQUESTED.load(Ordering::SeqCst) {
        HOOK_START_REQUESTED_COUNT.fetch_add(1, Ordering::SeqCst);
        set_status(SERVICE_START_PENDING, 0, 10_000);
        if rotate_files {
            for (path, copy) in [
                (stdout_path.as_deref(), stdout_copy_and_truncate),
                (stderr_path.as_deref(), stderr_copy_and_truncate),
            ] {
                if let Some(path) = path {
                    let _ = rotate_log_file(
                        path,
                        Duration::from_secs(rotate_seconds as u64),
                        rotate_size,
                        rotate_delay,
                        false,
                        copy,
                        false,
                    );
                }
            }
        }
        let redirected_stdin =
            match open_stdio_file(&service_name, "AppStdin", &child_environment, true) {
                Ok(file) => file.map(Stdio::from).unwrap_or_else(Stdio::null),
                Err(_) => {
                    set_status(SERVICE_STOPPED, 2, 0);
                    return;
                }
            };
        let mut stdout_file =
            match open_stdio_file(&service_name, "AppStdout", &child_environment, false) {
                Ok(file) => file,
                Err(_) => {
                    set_status(SERVICE_STOPPED, 2, 0);
                    return;
                }
            };
        let mut stderr_file =
            match open_stdio_file(&service_name, "AppStderr", &child_environment, false) {
                Ok(file) => file,
                Err(_) => {
                    set_status(SERVICE_STOPPED, 2, 0);
                    return;
                }
            };
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            break;
        }
        if matches!(
            invoke_hook(
                &service_name,
                "Start",
                "Pre",
                &child_environment,
                None,
                None,
                true,
                true,
            ),
            Ok(Some(99))
        ) {
            finish_async_hooks();
            set_status(SERVICE_STOPPED, 99, 0);
            return;
        }
        for (setting, file) in [
            ("AppStdout", stdout_file.as_mut()),
            ("AppStderr", stderr_file.as_mut()),
        ] {
            if let Some(file) = file
                && let Err(error) = file.seek(SeekFrom::End(0))
            {
                let reason = format!("unable to seek {setting} output after Start/Pre: {error}");
                report_event(
                    windows_sys::Win32::System::EventLog::EVENTLOG_ERROR_TYPE,
                    EVENT_START_SERVICE_FAILED,
                    &[&app, &service_name, &reason],
                );
                finish_async_hooks();
                set_status(SERVICE_STOPPED, 2, 0);
                return;
            }
        }
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            break;
        }
        let stdout_destination = if (timestamp_log || rotate_online) && stdout_path.is_some() {
            match (stdout_path.as_deref(), stdout_file.take()) {
                (Some(path), Some(file)) => {
                    match LogDestination::new(&service_name, "AppStdout", path.to_owned(), file) {
                        Ok(destination) => Some(destination),
                        Err(_) => {
                            set_status(SERVICE_STOPPED, 2, 0);
                            return;
                        }
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        let stderr_destination = if (timestamp_log || rotate_online) && stderr_path.is_some() {
            match (stderr_path.as_deref(), stderr_file.take()) {
                (Some(path), Some(file)) => {
                    match LogDestination::new(&service_name, "AppStderr", path.to_owned(), file) {
                        Ok(destination) => Some(destination),
                        Err(_) => {
                            set_status(SERVICE_STOPPED, 2, 0);
                            return;
                        }
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        let timestamp_stdout = timestamp_log && stdout_destination.is_some();
        let timestamp_stderr = timestamp_log && stderr_destination.is_some();
        let redirected_stdout = if stdout_destination.is_some() {
            Stdio::piped()
        } else {
            stdout_file
                .take()
                .map(Stdio::from)
                .unwrap_or_else(Stdio::null)
        };
        let redirected_stderr = if stderr_destination.is_some() {
            Stdio::piped()
        } else {
            stderr_file
                .take()
                .map(Stdio::from)
                .unwrap_or_else(Stdio::null)
        };
        let mut command = ProcessCommand::new(&app);
        command
            .args(&args)
            .stdin(redirected_stdin)
            .stdout(redirected_stdout)
            .stderr(redirected_stderr);
        if no_console {
            command.creation_flags(CREATE_NO_WINDOW);
        } else {
            // Keep the child in the console's default process group so a
            // CTRL_C_EVENT sent with group id 0 reaches its handler.
            let mut flags = 0;
            if read_reg_dword(&service_name, "AppStopMethodSkip", 0).unwrap_or(0)
                & STOP_SKIP_CONSOLE
                == 0
            {
                flags |= CREATE_NEW_CONSOLE;
            }
            command.creation_flags(flags);
        }
        let default_dir = Path::new(&app)
            .parent()
            .filter(|p| !p.as_os_str().is_empty());
        if let Some(dir) = directory.as_deref().map(Path::new).or(default_dir) {
            command.current_dir(dir);
        }
        if app_environment.is_some() {
            command.env_clear();
            for entry in &child_environment {
                if let Some((key, value)) = entry.split_once('=') {
                    command.env(key, value);
                }
            }
        } else if !extra_environment.is_empty() {
            for change in &extra_environment {
                if let Some((key, _)) = change.split_once('=') {
                    if let Some(value) = child_environment.iter().rev().find_map(|entry| {
                        entry
                            .split_once('=')
                            .filter(|(name, _)| name.eq_ignore_ascii_case(key))
                            .map(|(_, value)| value)
                    }) {
                        command.env(key, value);
                    }
                }
            }
        }
        let mut process = match command.spawn() {
            Ok(process) => process,
            Err(error) => {
                let error = error.to_string();
                report_event(
                    EVENTLOG_ERROR_TYPE,
                    EVENT_CREATEPROCESS_FAILED,
                    &[&service_name, &app, &error],
                );
                report_event(
                    EVENTLOG_ERROR_TYPE,
                    EVENT_START_SERVICE_FAILED,
                    &[
                        &app,
                        &service_name,
                        &SERVICE_LAUNCH_FAILURE_CODE.to_string(),
                    ],
                );
                set_status_with_service_exit(
                    SERVICE_STOPPED,
                    windows_sys::Win32::Foundation::ERROR_SERVICE_SPECIFIC_ERROR,
                    SERVICE_LAUNCH_FAILURE_CODE,
                    0,
                );
                return;
            }
        };
        let mut logger_threads = Vec::new();
        if let Some(destination) = stdout_destination.as_ref() {
            if let Some(pipe) = process.stdout.take() {
                let destination = destination.clone();
                logger_threads.push(thread::spawn(move || {
                    log_stream(pipe, destination, timestamp_stdout)
                }));
            }
        }
        if let Some(destination) = stderr_destination.as_ref() {
            if let Some(pipe) = process.stderr.take() {
                let destination = destination.clone();
                logger_threads.push(thread::spawn(move || {
                    log_stream(pipe, destination, timestamp_stderr)
                }));
            }
        }
        let launched = match assign_to_kill_on_close_job(process, kill_process_tree) {
            Ok(launched) => launched,
            Err(_) => {
                set_status(SERVICE_STOPPED, 2, 0);
                return;
            }
        };
        if let Ok(mut child) = CHILD_PROCESS.lock() {
            *child = Some(launched);
        }
        HOOK_START_COUNT.fetch_add(1, Ordering::SeqCst);
        HOOK_APPLICATION_START_MS.store(unix_time_millis(), Ordering::SeqCst);
        let app_pid = CHILD_PROCESS
            .lock()
            .ok()
            .and_then(|child| child.as_ref().map(|child| child.child.id()));
        let _ = invoke_hook(
            &service_name,
            "Start",
            "Post",
            &child_environment,
            app_pid,
            None,
            false,
            false,
        );
        if let Ok(child) = CHILD_PROCESS.lock() {
            if let Some(child) = child.as_ref() {
                let process = child.child.as_raw_handle() as HANDLE;
                unsafe {
                    SetPriorityClass(process, priority);
                }
                if let Some(affinity) = affinity.as_deref() {
                    if let Ok(mask) = parse_cpu_affinity(affinity) {
                        if mask != 0 {
                            let mut process_mask = 0usize;
                            let mut system_mask = 0usize;
                            if unsafe {
                                GetProcessAffinityMask(
                                    GetCurrentProcess(),
                                    &mut process_mask,
                                    &mut system_mask,
                                )
                            } != 0
                            {
                                let effective = mask as usize & system_mask;
                                if effective != 0 {
                                    unsafe {
                                        SetProcessAffinityMask(process, effective);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let child_started = Instant::now();
        set_status(SERVICE_RUNNING, 0, 0);
        let parameters = read_optional_reg_string(&service_name, "AppParameters")
            .ok()
            .flatten()
            .unwrap_or_default();
        report_event(
            windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
            EVENT_STARTED_SERVICE,
            &[
                &app,
                &parameters,
                &service_name,
                directory.as_deref().unwrap_or(""),
            ],
        );
        let mut child_exit_code = 1;
        let mut rotation_check = Instant::now();
        loop {
            if STOP_REQUESTED.load(Ordering::SeqCst) {
                break;
            }
            match POWER_HOOK_REQUESTED.swap(0, Ordering::SeqCst) {
                1 => {
                    let _ = invoke_hook(
                        &service_name,
                        "Power",
                        "Change",
                        &child_environment,
                        CHILD_PROCESS
                            .lock()
                            .ok()
                            .and_then(|child| child.as_ref().map(|child| child.child.id())),
                        None,
                        false,
                        false,
                    );
                }
                2 => {
                    let _ = invoke_hook(
                        &service_name,
                        "Power",
                        "Resume",
                        &child_environment,
                        CHILD_PROCESS
                            .lock()
                            .ok()
                            .and_then(|child| child.as_ref().map(|child| child.child.id())),
                        None,
                        false,
                        false,
                    );
                }
                _ => {}
            }
            let requested_rotation = ROTATE_REQUESTED.swap(false, Ordering::SeqCst);
            let check_thresholds = rotate_online
                && (rotate_seconds != 0 || rotate_size != 0)
                && rotation_check.elapsed() >= Duration::from_secs(1);
            if requested_rotation || check_thresholds {
                rotation_check = Instant::now();
                let _ = invoke_hook(
                    &service_name,
                    "Rotate",
                    "Pre",
                    &child_environment,
                    CHILD_PROCESS
                        .lock()
                        .ok()
                        .and_then(|child| child.as_ref().map(|child| child.child.id())),
                    None,
                    true,
                    false,
                );
                for (destination, path, copy) in [
                    (
                        stdout_destination.as_ref(),
                        stdout_path.as_deref(),
                        stdout_copy_and_truncate,
                    ),
                    (
                        stderr_destination.as_ref(),
                        stderr_path.as_deref(),
                        stderr_copy_and_truncate,
                    ),
                ] {
                    if let Some(destination) = destination {
                        let _ = destination.rotate(
                            Duration::from_secs(rotate_seconds as u64),
                            rotate_size,
                            rotate_delay,
                            requested_rotation,
                            copy,
                        );
                    } else if let Some(path) = path {
                        let _ = rotate_log_file(
                            path,
                            Duration::from_secs(rotate_seconds as u64),
                            rotate_size,
                            rotate_delay,
                            requested_rotation,
                            copy,
                            false,
                        );
                    }
                }
                let _ = invoke_hook(
                    &service_name,
                    "Rotate",
                    "Post",
                    &child_environment,
                    CHILD_PROCESS
                        .lock()
                        .ok()
                        .and_then(|child| child.as_ref().map(|child| child.child.id())),
                    None,
                    false,
                    false,
                );
            }
            if child_started.elapsed() >= Duration::from_millis(throttle_ms as u64) {
                throttle.mark_stable();
                HOOK_THROTTLE_COUNT.store(0, Ordering::SeqCst);
            }
            let exit_status = CHILD_PROCESS.lock().ok().and_then(|mut slot| {
                slot.as_mut()
                    .and_then(|c| c.child.try_wait().ok().flatten())
            });
            if let Some(status) = exit_status {
                child_exit_code = status.code().unwrap_or(1) as u32;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let app_pid = CHILD_PROCESS
            .lock()
            .ok()
            .and_then(|child| child.as_ref().map(|child| child.child.id()));
        if STOP_REQUESTED.load(Ordering::SeqCst) && !stop_hook_invoked {
            let _ = invoke_hook(
                &service_name,
                "Stop",
                "Pre",
                &child_environment,
                app_pid,
                None,
                true,
                false,
            );
            stop_hook_invoked = true;
        }
        if let Ok(mut child) = CHILD_PROCESS.lock() {
            if let Some(mut proc) = child.take() {
                let stop_requested = STOP_REQUESTED.load(Ordering::SeqCst);
                if stop_requested {
                    let _ = stop_child(&service_name, &mut proc, no_console);
                }
                if let Ok(status) = proc.child.wait() {
                    if STOP_REQUESTED.load(Ordering::SeqCst) {
                        child_exit_code = status.code().unwrap_or(1) as u32;
                    }
                }
            }
        }
        if logger_threads
            .into_iter()
            .any(|logger| logger.join().map_or(true, |result| result.is_err()))
        {
            child_exit_code = 2;
        }
        if !STOP_REQUESTED.load(Ordering::SeqCst) {
            report_event(
                windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
                EVENT_ENDED_SERVICE,
                &[&app, &service_name, &child_exit_code.to_string()],
            );
        }
        let app_start_ms = HOOK_APPLICATION_START_MS.load(Ordering::SeqCst);
        if app_start_ms != 0 {
            HOOK_APPLICATION_RUNTIME_MS.store(
                unix_time_millis().saturating_sub(app_start_ms),
                Ordering::SeqCst,
            );
        }
        HOOK_EXIT_COUNT.fetch_add(1, Ordering::SeqCst);
        let _ = invoke_hook(
            &service_name,
            "Exit",
            "Post",
            &child_environment,
            None,
            Some(child_exit_code),
            false,
            false,
        );
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            break;
        }
        let (action, inherited_default) = match exit_action(&service_name, child_exit_code) {
            Ok(action) => action,
            Err(_) => {
                set_status(SERVICE_STOPPED, 2, 0);
                return;
            }
        };
        let action = normalize_exit_action(child_exit_code, action, inherited_default);
        let exit_code = child_exit_code.to_string();
        match action {
            ExitAction::Ignore => {
                report_event(
                    windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
                    EVENT_EXIT_IGNORE,
                    &[&service_name, &exit_code, "Ignore", &app],
                );
                while !STOP_REQUESTED.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(250));
                }
                break;
            }
            ExitAction::Exit => {
                report_event(
                    windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
                    EVENT_EXIT,
                    &[&service_name, &exit_code, "Exit"],
                );
                finish_async_hooks();
                set_status(SERVICE_STOPPED, child_exit_code, 0);
                return;
            }
            ExitAction::Suicide => {
                report_event(
                    windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
                    EVENT_EXIT_UNCLEAN,
                    &[&service_name, &exit_code, "Suicide"],
                );
                finish_async_hooks();
                // NSSM deliberately exits without reporting SERVICE_STOPPED so
                // SCM treats this as a service crash and can run recovery actions.
                std::process::exit(child_exit_code.max(1) as i32);
            }
            ExitAction::Restart => {
                report_event(
                    windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
                    EVENT_EXIT_RESTART,
                    &[&service_name, &exit_code, "Restart", &app],
                );
            }
        }
        let delay = throttle.after_exit();
        HOOK_THROTTLE_COUNT.store(throttle.counter(), Ordering::SeqCst);
        if delay > 0 {
            CONTINUE_REQUESTED.store(false, Ordering::SeqCst);
            set_status(SERVICE_PAUSED, 0, delay);
            let deadline = Instant::now() + Duration::from_millis(delay as u64);
            while !STOP_REQUESTED.load(Ordering::SeqCst) && Instant::now() < deadline {
                if CONTINUE_REQUESTED.swap(false, Ordering::SeqCst) {
                    throttle.mark_stable();
                    HOOK_THROTTLE_COUNT.store(0, Ordering::SeqCst);
                    break;
                }
                thread::sleep(
                    Duration::from_millis(25)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
    }
    if STOP_REQUESTED.load(Ordering::SeqCst) && !stop_hook_invoked {
        let _ = invoke_hook(
            &service_name,
            "Stop",
            "Pre",
            &child_environment,
            None,
            None,
            true,
            false,
        );
    }
    finish_async_hooks();
    set_status(SERVICE_STOPPED, 0, 0);
}

pub fn run_service(name: &str) -> Result<(), String> {
    let _ = ensure_event_source();
    let service_name = wide(OsStr::new(name));
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: service_name.as_ptr() as *mut u16,
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        let error = io::Error::last_os_error().to_string();
        report_event(EVENTLOG_ERROR_TYPE, EVENT_DISPATCHER_FAILED, &[&error]);
        Err(error)
    } else {
        Ok(())
    }
}

fn status_code(name: &str) -> Result<u8, String> {
    let handle = match open_service(name, SERVICE_QUERY_STATUS) {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("{name}: {error}");
            return Ok(0);
        }
    };
    let current_state = match state(handle.0) {
        Ok(current_state) => current_state,
        Err(error) => {
            eprintln!("{name}: {error}");
            return Ok(0);
        }
    };
    println!("{}", state_name(current_state));
    Ok(current_state.min(u8::MAX as u32) as u8)
}

pub fn execute(command: Command) -> Result<u8, String> {
    if let Command::StatusCode(name) = command {
        return status_code(&name);
    }

    execute_service_command(command).map(|()| 0)
}

fn execute_service_command(command: Command) -> Result<(), String> {
    let _ = ensure_event_source();
    match command {
        Command::Install {
            service,
            application,
            args,
        } => install(&service, application.as_deref(), &args),
        Command::Start(name) => start(&name),
        Command::Stop(name) => stop(&name),
        Command::Pause(name) => control_simple(&name, SERVICE_CONTROL_PAUSE),
        Command::Continue(name) => control_simple(&name, SERVICE_CONTROL_CONTINUE),
        Command::Rotate(name) => rotate(&name),
        Command::Restart(name) => {
            stop(&name)?;
            start(&name)
        }
        Command::Status(name) => {
            let s = state(open_service(&name, SERVICE_QUERY_STATUS)?.0)?;
            println!("{}", state_name(s));
            Ok(())
        }
        Command::StatusCode(name) => status_code(&name).map(|_| ()),
        Command::Remove { service, confirm } => remove(&service, confirm),
        Command::Get {
            service,
            parameter,
            extra,
        } => get_setting(&service, &parameter, &extra),
        Command::Set {
            service,
            parameter,
            values,
        } => set_setting(&service, &parameter, &values),
        Command::Reset {
            service,
            parameter,
            extra,
        } => reset_setting(&service, &parameter, &extra),
        Command::Unset {
            service,
            parameter,
            extra,
        } => reset_setting(&service, &parameter, &extra),
        Command::Dump { service, new_name } => dump_service(&service, new_name.as_deref()),
        Command::List { all } => list_services(all),
        Command::Processes(names) => processes(&names),
        _ => Err(format!("command is not implemented yet: {command:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PBT_APMPOWERSTATUSCHANGE, PBT_APMRESUMEAUTOMATIC, hook_deadline_ms, parse_u32,
        power_hook_action, priority_name, priority_value, valid_hook,
    };

    #[test]
    fn hook_deadlines_match_service_status_and_general_hook_limits() {
        assert_eq!(hook_deadline_ms("Start", "Pre"), 20_000);
        assert_eq!(hook_deadline_ms("Stop", "Pre"), 20_000);
        assert_eq!(hook_deadline_ms("Rotate", "Pre"), 60_000);
        assert_eq!(hook_deadline_ms("Start", "Post"), 60_000);
        assert_eq!(hook_deadline_ms("Exit", "Post"), 60_000);
        assert_eq!(hook_deadline_ms("start", "pre"), 20_000);
    }

    #[test]
    fn power_events_map_only_supported_windows_broadcasts_to_hooks() {
        assert_eq!(power_hook_action(PBT_APMPOWERSTATUSCHANGE), Some(1));
        assert_eq!(power_hook_action(PBT_APMRESUMEAUTOMATIC), Some(2));
        assert_eq!(power_hook_action(0), None);
        assert_eq!(power_hook_action(4), None);
        assert_eq!(power_hook_action(7), None);
        assert_eq!(power_hook_action(0xffff), None);
    }

    #[test]
    fn unsigned_setting_values_accept_decimal_and_hex_but_reject_invalid_numbers() {
        assert_eq!(parse_u32("42"), Ok(42));
        assert_eq!(parse_u32(" 0xff "), Ok(255));
        assert_eq!(parse_u32("0XFFFFFFFF"), Ok(u32::MAX));
        assert!(parse_u32("").is_err());
        assert!(parse_u32("-1").is_err());
        assert!(parse_u32("0x").is_err());
        assert!(parse_u32("4294967296").is_err());
    }

    #[test]
    fn priority_names_round_trip_to_windows_priority_class_values() {
        for (name, value) in [
            ("IDLE_PRIORITY_CLASS", 0x40),
            ("BELOW_NORMAL_PRIORITY_CLASS", 0x4000),
            ("NORMAL_PRIORITY_CLASS", 0x20),
            ("ABOVE_NORMAL_PRIORITY_CLASS", 0x8000),
            ("HIGH_PRIORITY_CLASS", 0x80),
            ("REALTIME_PRIORITY_CLASS", 0x100),
        ] {
            assert_eq!(priority_value(name), Ok(value));
            assert_eq!(priority_value(&name.to_ascii_lowercase()), Ok(value));
            assert_eq!(priority_name(value), Ok(name));
        }
        assert!(priority_value("INVALID_PRIORITY_CLASS").is_err());
        assert!(priority_name(0).is_err());
    }

    #[test]
    fn hook_configuration_accepts_only_supported_event_action_pairs() {
        for (event, action) in [
            ("Exit", "Post"),
            ("Power", "Change"),
            ("Power", "Resume"),
            ("Rotate", "Pre"),
            ("Rotate", "Post"),
            ("Start", "Pre"),
            ("Start", "Post"),
            ("Stop", "Pre"),
        ] {
            assert!(
                valid_hook(event, action),
                "{event}/{action} should be valid"
            );
            assert!(
                valid_hook(&event.to_ascii_lowercase(), &action.to_ascii_lowercase()),
                "{event}/{action} should be case-insensitive"
            );
        }
        for (event, action) in [
            ("Exit", "Pre"),
            ("Power", "Pre"),
            ("Rotate", "Change"),
            ("Start", "Change"),
            ("Stop", "Post"),
            ("Unknown", "Post"),
        ] {
            assert!(
                !valid_hook(event, action),
                "{event}/{action} should be invalid"
            );
        }
    }
}
