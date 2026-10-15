use nssm_rs::{
    Command, ExitAction, LaunchFailureDisposition, MANAGED_SETTINGS, SettingKind, StopMethod,
    compose_child_environment, edit_dependencies, edit_environment, expand_environment_value,
    format_cpu_affinity_mask, log_rotation_due, managed_setting, normalize_cpu_affinity,
    normalize_exit_action, parse_cpu_affinity, quote_windows_arg, rotated_log_path,
    split_windows_args,
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
        CloseHandle, ERROR_NO_MORE_FILES, ERROR_NO_MORE_ITEMS, FILETIME, HANDLE, HWND,
        INVALID_HANDLE_VALUE, LPARAM, NO_ERROR, WAIT_OBJECT_0, WPARAM,
    },
    Globalization::{
        CSTR_EQUAL, CompareStringOrdinal, GetUserDefaultUILanguage, WideCharToMultiByte,
    },
    System::Console::{
        AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, GetConsoleMode,
        GetConsoleWindow, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
        SetConsoleOutputCP, WriteConsoleW,
    },
    System::{
        Diagnostics::{Debug::FormatMessageW, ToolHelp::*},
        EventLog::{
            DeregisterEventSource, EVENTLOG_ERROR_TYPE, EVENTLOG_INFORMATION_TYPE,
            RegisterEventSourceW, ReportEventW,
        },
        JobObjects::*,
        Registry::*,
        Services::*,
        SystemInformation::{GetSystemTimeAsFileTime, GetWindowsDirectoryW},
        Threading::*,
    },
    UI::Shell::{IsUserAnAdmin, SHELLEXECUTEINFOW, ShellExecuteExW},
    UI::WindowsAndMessaging::{
        ENDSESSION_CLOSEAPP, ENDSESSION_CRITICAL, ENDSESSION_LOGOFF, EnumWindows,
        GetWindowThreadProcessId, PBT_APMPOWERSTATUSCHANGE, PBT_APMRESUMEAUTOMATIC, PostMessageW,
        PostThreadMessageW, SW_SHOW, WM_CLOSE, WM_ENDSESSION, WM_QUIT,
    },
};

#[cfg(test)]
use windows_sys::Win32::Foundation::SetLastError;

const DELETE_ACCESS: u32 = 0x0001_0000;
const STOP_SKIP_CONSOLE: u32 = 1;
const STOP_SKIP_TERMINATE: u32 = 8;
const SERVICE_INTERACTIVE_PROCESS_FLAG: u32 = 0x100;
const EVENT_DISPATCHER_FAILED: u32 = 0xc000_03e9;
const EVENT_OPENSCMANAGER_FAILED: u32 = 0xc000_03ea;
const EVENT_START_SERVICE_FAILED: u32 = 0xc000_03ee;
const EVENT_RESTART_SERVICE_FAILED: u32 = 0x8000_03ef;
const EVENT_STARTED_SERVICE: u32 = 0x4000_03f0;
const EVENT_CREATEPROCESS_FAILED: u32 = 0xc000_03f2;
const EVENT_CREATEFILE_FAILED: u32 = 0xc000_0419;
const EVENT_DUPLICATEHANDLE_FAILED: u32 = 0xc000_041a;
const EVENT_GET_OUTPUT_HANDLES_FAILED: u32 = 0xc000_041b;
const EVENT_TERMINATEPROCESS: u32 = 0x4000_03f3;
const EVENT_PROCESS_ALREADY_STOPPED: u32 = 0x4000_03f4;
const EVENT_ENDED_SERVICE: u32 = 0x4000_03f5;
const EVENT_EXIT_RESTART: u32 = 0x4000_03f6;
const EVENT_EXIT_IGNORE: u32 = 0x4000_03f7;
const EVENT_EXIT: u32 = 0x4000_03f8;
const EVENT_EXIT_UNCLEAN: u32 = 0x4000_03fc;
const EVENT_GRACEFUL_SUICIDE: u32 = 0x4000_03fd;
const EVENT_RESET_THROTTLE: u32 = 0x4000_040b;
const EVENT_THROTTLED: u32 = 0x8000_040a;
const EVENT_RESTART_DELAY: u32 = 0x4000_0430;
const EVENT_ROTATED: u32 = 0x4000_0435;
const EVENT_AWAITING_SINGLE_HANDLE: u32 = 0x4000_0436;
const EVENT_HOOK_CREATEPROCESS_FAILED: u32 = 0xc000_0438;
const EVENT_GET_HOOK_FAILED: u32 = 0xc000_0439;
const EVENT_NO_FLAGS: u32 = 0x8000_0405;
const EVENT_NO_DIRECTORY: u32 = 0x8000_0406;
const EVENT_DIRECTORY_FAILED: u32 = 0xc000_0407;
const EVENT_KILLING: u32 = 0x4000_03ff;
const EVENT_GET_PARAMETERS_FAILED: u32 = 0xc000_03ec;
const EVENT_REGISTER_HANDLER_FAILED: u32 = 0xc000_03ed;
const EVENT_PROCESS_SNAPSHOT_FAILED: u32 = 0xc000_0400;
const EVENT_PROCESS_ENUMERATE_FAILED: u32 = 0xc000_0401;
const EVENT_OPEN_PROCESS_FAILED: u32 = 0xc000_0402;
const EVENT_GET_PROCESS_TIMES_FAILED: u32 = 0xc000_0414;
const EVENT_ROTATE_FILE_FAILED: u32 = 0xc000_0427;
const EVENT_SERVICE_CONTROL_HANDLED: u32 = 0x4000_0410;
const EVENT_SERVICE_CONTROL_NOT_HANDLED: u32 = 0x4000_0411;
const EVENT_SERVICE_CONTROL_UNKNOWN: u32 = 0x4000_0412;
const EVENT_KILL_PROCESS_TREE: u32 = 0x4000_0403;
const EVENT_BOGUS_PRIORITY: u32 = 0x4000_042a;
const EVENT_BOGUS_AFFINITY_MASK: u32 = 0x8000_042b;
const EVENT_EFFECTIVE_AFFINITY_MASK: u32 = 0x8000_042c;
const EVENT_GET_PROCESS_AFFINITY_MASK_FAILED: u32 = 0x8000_042d;
const EVENT_SET_PROCESS_AFFINITY_MASK_FAILED: u32 = 0xc000_042e;
const EVENT_BOGUS_STOP_METHOD_SKIP: u32 = 0x8000_041c;
const EVENT_PRESTART_HOOK_ABORT: u32 = 0xc000_0437;
const EVENT_PROCESS_STILL_ACTIVE: u32 = 0x8000_041d;
const EVENT_SERVICE_CONFIG_FAILURE_ACTIONS_FAILED: u32 = 0xc000_0413;
const EVENT_QUERYVALUE_FAILED: u32 = 0xc000_03fa;
const PRIORITY_CLASS_MASK: u32 = 0x0000_c1e0;
const SERVICE_LAUNCH_FAILURE_CODE: u32 = 3;
const EVENT_SOURCE: &str = "nssm";
const SERVICE_KEY_PREFIX: &str = "SYSTEM\\CurrentControlSet\\Services\\";
// NSSM's MSVC CRT uses the default "C" locale for redirected wide output.
const NSSM_C_LOCALE_CODE_PAGE: u32 = 1252;
const WC_NO_BEST_FIT_CHARS: u32 = 0x0000_0400;
const CP_UTF8: u32 = 65001;

fn launched_from_existing_console() -> bool {
    let console = unsafe { GetConsoleWindow() };
    if console.is_null() {
        return false;
    }
    let mut console_process = 0;
    if unsafe { GetWindowThreadProcessId(console, &mut console_process) } == 0 {
        return false;
    }
    unsafe { GetCurrentProcessId() != console_process }
}

fn write_cli_output(text: &str, is_error: bool) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let utf8_console_mode = launched_from_existing_console();
    if utf8_console_mode {
        unsafe {
            SetConsoleOutputCP(CP_UTF8);
        }
    }
    let handle = unsafe {
        GetStdHandle(if is_error {
            STD_ERROR_HANDLE
        } else {
            STD_OUTPUT_HANDLE
        })
    };
    if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
        let mut mode = 0;
        if unsafe { GetConsoleMode(handle, &mut mode) } != 0 {
            let mut written = 0;
            unsafe {
                WriteConsoleW(
                    handle,
                    wide.as_ptr(),
                    wide.len() as u32,
                    &mut written,
                    ptr::null(),
                );
            }
            return;
        }
    }

    let (code_page, conversion_flags) = if utf8_console_mode {
        (CP_UTF8, 0)
    } else {
        (NSSM_C_LOCALE_CODE_PAGE, WC_NO_BEST_FIT_CHARS)
    };

    let required = unsafe {
        WideCharToMultiByte(
            code_page,
            conversion_flags,
            wide.as_ptr(),
            wide.len() as i32,
            ptr::null_mut(),
            0,
            ptr::null(),
            ptr::null_mut(),
        )
    };
    if required <= 0 {
        return;
    }
    let mut bytes = vec![0; required as usize];
    let written = unsafe {
        WideCharToMultiByte(
            code_page,
            conversion_flags,
            wide.as_ptr(),
            wide.len() as i32,
            bytes.as_mut_ptr(),
            required,
            ptr::null(),
            ptr::null_mut(),
        )
    };
    if written <= 0 {
        return;
    }
    if is_error {
        let mut output = io::stderr().lock();
        let _ = output.write_all(&bytes[..written as usize]);
    } else {
        let mut output = io::stdout().lock();
        let _ = output.write_all(&bytes[..written as usize]);
    }
}

macro_rules! cli_println {
    ($($argument:tt)*) => {{
        let mut line = format!($($argument)*);
        line.push('\n');
        write_cli_output(&line, false);
    }};
}

macro_rules! cli_eprintln {
    ($($argument:tt)*) => {{
        let mut line = format!($($argument)*);
        line.push('\n');
        write_cli_output(&line, true);
    }};
}

static SERVICE_STATUS_HANDLE_VALUE: AtomicIsize = AtomicIsize::new(0);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static STOP_WAIT_CHECKPOINT: AtomicU32 = AtomicU32::new(0);
static STOP_WAIT_HINT: AtomicU32 = AtomicU32::new(10_000);
static ROTATE_REQUESTED: AtomicBool = AtomicBool::new(false);
static ROTATE_ENABLED: AtomicBool = AtomicBool::new(false);
static CONTINUE_REQUESTED: AtomicBool = AtomicBool::new(false);
static HOOK_START_REQUESTED_COUNT: AtomicU32 = AtomicU32::new(0);
static HOOK_START_COUNT: AtomicU32 = AtomicU32::new(0);
static HOOK_EXIT_COUNT: AtomicU32 = AtomicU32::new(0);
static HOOK_THROTTLE_COUNT: AtomicU32 = AtomicU32::new(0);
static HOOK_LAST_CONTROL: AtomicU32 = AtomicU32::new(0);
static HOOK_SERVICE_START_MS: AtomicU64 = AtomicU64::new(0);
static HOOK_APPLICATION_START_MS: AtomicU64 = AtomicU64::new(0);
static HOOK_APPLICATION_RUNTIME_MS: AtomicU64 = AtomicU64::new(0);
static HOOK_LAST_APPLICATION_EXIT_CODE: AtomicU32 = AtomicU32::new(0);
static CHILD_PROCESS: Mutex<Option<ManagedChild>> = Mutex::new(None);
static SERVICE_CHILD_ENVIRONMENT: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ASYNC_HOOKS: Mutex<Vec<Child>> = Mutex::new(Vec::new());

const NATIVE_SETTING_NAMES: &[&str] = &[
    "DependOnGroup",
    "DependOnService",
    "Description",
    "DisplayName",
    "Environment",
    "ImagePath",
    "ObjectName",
    "Name",
    "Start",
    "Type",
];

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

#[derive(Debug)]
struct RotationFailure {
    operation: &'static str,
    target: String,
    message: String,
}

impl RotationFailure {
    fn from_io(operation: &'static str, target: &str, error: &io::Error) -> Self {
        Self {
            operation,
            target: target.to_owned(),
            message: windows_error_message(error),
        }
    }

    fn from_message(operation: &'static str, target: &str, message: impl Into<String>) -> Self {
        Self {
            operation,
            target: target.to_owned(),
            message: message.into(),
        }
    }
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
    ) -> Result<Option<std::path::PathBuf>, RotationFailure> {
        let mut file_guard = self.file.lock().map_err(|_| {
            RotationFailure::from_message(
                "rotate_log_file()",
                &self.path,
                "log destination lock poisoned",
            )
        })?;
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RotationFailure::from_io("CreateFile()", &self.path, &error));
            }
        };
        let modified = metadata.modified().map_err(|error| {
            RotationFailure::from_io("GetFileInformationByHandle()", &self.path, &error)
        })?;
        if !log_rotation_due(
            metadata.len(),
            modified.elapsed().unwrap_or_default(),
            minimum_age,
            minimum_size,
            force,
        ) {
            return Ok(None);
        }
        let rotated = rotated_log_path(Path::new(&self.path), std::time::SystemTime::now())
            .ok_or_else(|| {
                RotationFailure::from_message(
                    "rotated_filename()",
                    &self.path,
                    "could not create rotated filename",
                )
            })?;
        file_guard
            .as_mut()
            .ok_or_else(|| {
                RotationFailure::from_message(
                    "FlushFileBuffers()",
                    &self.path,
                    "log destination is closed",
                )
            })?
            .flush()
            .map_err(|error| RotationFailure::from_io("FlushFileBuffers()", &self.path, &error))?;

        if copy_and_truncate {
            fs::copy(&self.path, &rotated).map_err(|error| {
                RotationFailure::from_io("CopyFile()", &rotated.to_string_lossy(), &error)
            })?;
            drop(file_guard);
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            let mut file_guard = self.file.lock().map_err(|_| {
                RotationFailure::from_message(
                    "SetEndOfFile()",
                    &self.path,
                    "log destination lock poisoned",
                )
            })?;
            file_guard
                .as_mut()
                .ok_or_else(|| {
                    RotationFailure::from_message(
                        "SetEndOfFile()",
                        &self.path,
                        "log destination is closed",
                    )
                })?
                .set_len(0)
                .map_err(|error| RotationFailure::from_io("SetEndOfFile()", &self.path, &error))?;
        } else {
            drop(file_guard.take());
            if let Err(error) = fs::rename(&self.path, &rotated) {
                self.disposition.store(4, Ordering::SeqCst);
                *file_guard = Some(self.reopen(4).map_err(|reopen| {
                    RotationFailure::from_message(
                        "MoveFile()",
                        &rotated.to_string_lossy(),
                        format!("{error}; reopen failed: {reopen}"),
                    )
                })?);
                return Err(RotationFailure::from_io(
                    "MoveFile()",
                    &rotated.to_string_lossy(),
                    &error,
                ));
            }
            *file_guard = Some(
                self.reopen(self.disposition.load(Ordering::SeqCst))
                    .map_err(|error| {
                        RotationFailure::from_io("CreateFile()", &self.path, &error)
                    })?,
            );
        }
        Ok(Some(rotated))
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
    if !data.len().is_multiple_of(2) {
        return Err("registry string has an odd byte length".into());
    }
    let chars: Vec<u16> = data
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|c| *c != 0)
        .collect();
    Ok(String::from_utf16_lossy(&chars))
}

fn decode_multi_string(data: &[u8]) -> Result<Vec<String>, String> {
    if !data.len().is_multiple_of(2) {
        return Err("registry multi-string has an odd byte length".into());
    }
    let chars: Vec<u16> = data
        .chunks(2)
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

fn format_cli_multi_string(values: &[String]) -> String {
    // NSSM builds CRLF-separated REG_MULTI_SZ text, then writes it through the
    // Windows text-mode CRT, which translates the trailing LF to CRLF.
    values.join("\r\r\n")
}

fn service_type_name(service_type: u32) -> &'static str {
    match service_type {
        SERVICE_KERNEL_DRIVER => "SERVICE_KERNEL_DRIVER",
        SERVICE_FILE_SYSTEM_DRIVER => "SERVICE_FILE_SYSTEM_DRIVER",
        SERVICE_WIN32_OWN_PROCESS => "SERVICE_WIN32_OWN_PROCESS",
        SERVICE_WIN32_SHARE_PROCESS => "SERVICE_WIN32_SHARE_PROCESS",
        value if value == (SERVICE_WIN32_OWN_PROCESS | SERVICE_INTERACTIVE_PROCESS_FLAG) => {
            "SERVICE_INTERACTIVE_PROCESS"
        }
        value if value == (SERVICE_WIN32_SHARE_PROCESS | SERVICE_INTERACTIVE_PROCESS_FLAG) => {
            "SERVICE_WIN32_SHARE_PROCESS|SERVICE_INTERACTIVE_PROCESS"
        }
        _ => "?",
    }
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
    spec: &nssm_rs::SettingSpec,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerHookAction {
    Change,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PowerHookInvocation {
    action: PowerHookAction,
    control: u32,
    synchronous: bool,
    cancel_on_stop: bool,
}

impl PowerHookAction {
    fn hook_action(self) -> &'static str {
        match self {
            Self::Change => "Change",
            Self::Resume => "Resume",
        }
    }
}

fn power_hook_action(event_type: u32) -> Option<PowerHookAction> {
    match event_type {
        PBT_APMRESUMEAUTOMATIC => Some(PowerHookAction::Resume),
        PBT_APMPOWERSTATUSCHANGE => Some(PowerHookAction::Change),
        _ => None,
    }
}

fn power_hook_invocation(event_type: u32) -> Option<PowerHookInvocation> {
    Some(PowerHookInvocation {
        action: power_hook_action(event_type)?,
        control: SERVICE_CONTROL_POWEREVENT,
        synchronous: false,
        cancel_on_stop: false,
    })
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

fn valid_priority_class(value: u32) -> bool {
    value & PRIORITY_CLASS_MASK == value
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
        cli_println!("{display}\r");
        return Ok(());
    }
    if spec.kind == SettingKind::Hook {
        let hook =
            additional.ok_or_else(|| "AppEvents requires an event/action pair".to_owned())?;
        cli_println!("{}\r", read_hook(service, hook)?.unwrap_or_default());
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
                format_cli_multi_string(&values)
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
        SettingKind::String => {
            let value = raw
                .map(|(_, bytes)| decode_wide(&bytes))
                .transpose()?
                .or_else(|| spec.default.map(str::to_owned))
                .unwrap_or_default();
            if parameter.eq_ignore_ascii_case("AppAffinity") {
                normalize_cpu_affinity(&value)
                    .map_err(|error| error.to_string())?
                    .unwrap_or_else(|| "All".to_owned())
            } else {
                value
            }
        }
    };
    cli_println!("{display}\r");
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
            let value = if parameter.eq_ignore_ascii_case("AppAffinity") {
                match normalize_cpu_affinity(&value).map_err(|error| error.to_string())? {
                    Some(value) => value,
                    None => return delete_raw_value(key.0, value_name.as_deref()),
                }
            } else {
                value
            };
            let kind = if parameter.eq_ignore_ascii_case("AppAffinity") {
                REG_SZ
            } else {
                REG_EXPAND_SZ
            };
            if spec
                .default
                .is_some_and(|default| !default.is_empty() && default == value)
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
    let service = canonical_service_name(service).map_err(|error| error.to_string())?;
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
                format_cli_multi_string(&values)
            };
            cli_println!("{display}\r");
            Ok(())
        }
        "name" => {
            let canonical = canonical_service_name(service).unwrap_or_else(|_| service.to_owned());
            cli_println!("{canonical}\r");
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
                    format_cli_multi_string(&selected)
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
                "type" => service_type_name(config.dwServiceType).into(),
                _ => unreachable!(),
            };
            cli_println!("{value}\r");
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
                        if !nssm_rs::object_account_requires_password(account, service) {
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

fn quote_dump_arg(argument: &str) -> String {
    let needs_escape = |character| matches!(character, '"' | '&' | '%' | '^' | '<' | '>' | '|');
    let needs_quote = |character| {
        matches!(character, ' ' | '\t' | '\n' | '\u{000b}' | '"' | '*') || needs_escape(character)
    };
    let mut escaped = false;
    let mut quoted = false;
    for character in argument.chars() {
        if needs_escape(character) {
            escaped = true;
            quoted = true;
            break;
        }
        if needs_quote(character) {
            quoted = true;
        }
    }
    if !quoted {
        return argument.to_owned();
    }

    let mut output = String::new();
    if escaped {
        output.push('^');
    }
    output.push('"');
    let characters = argument.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < characters.len() {
        let mut backslashes = 0;
        while index < characters.len() && characters[index] == '\\' {
            backslashes += 1;
            index += 1;
        }
        if index == characters.len() {
            for _ in 0..backslashes * 2 {
                if escaped {
                    output.push('^');
                }
                output.push('\\');
            }
            break;
        }
        let character = characters[index];
        if character == '"' {
            for _ in 0..backslashes * 2 + 1 {
                if escaped {
                    output.push('^');
                }
                output.push('\\');
            }
            if escaped && needs_escape(character) {
                output.push('^');
            }
            output.push(character);
        } else {
            for _ in 0..backslashes {
                if escaped {
                    output.push('^');
                }
                output.push('\\');
            }
            if escaped && needs_escape(character) {
                output.push('^');
            }
            output.push(character);
        }
        index += 1;
    }
    if escaped {
        output.push('^');
    }
    output.push('"');
    output
}

fn dump_set_line_values(
    executable: &str,
    service: &str,
    parameter: &str,
    additional: Option<&str>,
    values: &[&str],
) {
    let mut arguments = vec![
        quote_dump_arg(executable),
        "set".into(),
        quote_dump_arg(service),
        quote_dump_arg(parameter),
    ];
    if let Some(additional) = additional {
        arguments.push(quote_dump_arg(additional));
    }
    arguments.extend(values.iter().map(|value| quote_dump_arg(value)));
    cli_println!("{}\r", arguments.join(" "));
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
                    let value = if spec.name.eq_ignore_ascii_case("AppPriority") {
                        priority_name(value)?.to_owned()
                    } else {
                        value.to_string()
                    };
                    dump_set_line(executable, target, spec.name, None, &value);
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
    let display_name =
        if !target.eq_ignore_ascii_case(source) && display_name.eq_ignore_ascii_case(source) {
            target.to_owned()
        } else {
            display_name
        };
    if !display_name.is_empty() {
        dump_set_line(executable, target, "DisplayName", None, &display_name);
    }
    let account = wide_pointer(config.lpServiceStartName);
    if !account.is_empty() {
        let password = if account.eq_ignore_ascii_case("LocalSystem")
            || account.eq_ignore_ascii_case("SYSTEM")
            || account.eq_ignore_ascii_case("NT AUTHORITY\\SYSTEM")
            || account.eq_ignore_ascii_case("NT AUTHORITY\\LocalService")
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
    let service_type = service_type_name(config.dwServiceType);
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

fn dump_service(source: &str, target: Option<&str>) -> Result<(), (u8, String)> {
    let source = canonical_service_name(source).map_err(|error| (3, error.to_string()))?;
    let source = source.as_str();
    let _service = open_service(source, SERVICE_QUERY_CONFIG | SERVICE_CHANGE_CONFIG)
        .map_err(|error| (3, dump_open_service_error(&error)))?;
    let application = read_optional_reg_string(source, "Application")
        .map_err(|error| (4, error))?
        .unwrap_or_default();
    let target = target.unwrap_or(source);
    let executable = std::env::current_exe().map_err(|error| (6, error.to_string()))?;
    let executable = executable.to_string_lossy();
    let install = [
        quote_dump_arg(&executable),
        "install".into(),
        quote_dump_arg(target),
        quote_dump_arg(&application),
    ];
    cli_println!("{}\r", install.join(" "));
    if !application.is_empty() {
        dump_managed_settings(&executable, source, target).map_err(|error| (1, error))?;
    }
    dump_native_settings(&executable, source, target).map_err(|error| (1, error))
}

fn dump_open_service_error(error: &str) -> String {
    if error.ends_with("(os error 5)") {
        let system_error = windows_error_message(&io::Error::from_raw_os_error(5));
        format!("Can't open service!\r\r\nOpenService(): {system_error}\r\r\n\r\r\n")
    } else {
        error.to_owned()
    }
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

fn assign_to_job(mut child: Child, kill_process_tree: bool) -> Result<ManagedChild, String> {
    if !kill_process_tree {
        return Ok(ManagedChild { child, job: 0 });
    }
    let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if job.is_null() {
        let _ = child.kill();
        return Err(io::Error::last_os_error().to_string());
    }
    if unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) } == 0 {
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

fn resume_suspended_child(process_id: u32) -> Result<(), String> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(windows_error_message(&io::Error::last_os_error()));
    }
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    let thread_id = loop {
        if !more {
            break None;
        }
        if entry.th32OwnerProcessID == process_id {
            break Some(entry.th32ThreadID);
        }
        more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    };
    unsafe { CloseHandle(snapshot) };
    let Some(thread_id) = thread_id else {
        return Err(format!("no primary thread found for process {process_id}"));
    };
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if thread.is_null() {
        return Err(windows_error_message(&io::Error::last_os_error()));
    }
    let result = unsafe { ResumeThread(thread) };
    unsafe { CloseHandle(thread) };
    if result == u32::MAX {
        return Err(windows_error_message(&io::Error::last_os_error()));
    }
    Ok(())
}

fn configure_child_affinity(service: &str, process: HANDLE, affinity: Option<&str>) {
    let Some(affinity) = affinity else {
        return;
    };
    let Ok(requested) = parse_cpu_affinity(affinity) else {
        return;
    };
    if requested == 0 {
        return;
    }
    let (mut process_mask, mut system_mask) = (0usize, 0usize);
    let mask =
        if unsafe { GetProcessAffinityMask(process, &mut process_mask, &mut system_mask) } != 0 {
            requested as usize & system_mask
        } else {
            let error = windows_error_message(&io::Error::last_os_error());
            report_event(
                windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE,
                EVENT_GET_PROCESS_AFFINITY_MASK_FAILED,
                &[service, &error],
            );
            requested as usize
        };
    if unsafe { SetProcessAffinityMask(process, mask) } == 0 {
        let error = windows_error_message(&io::Error::last_os_error());
        report_event(
            EVENTLOG_ERROR_TYPE,
            EVENT_SET_PROCESS_AFFINITY_MASK_FAILED,
            &[service, &error],
        );
    }
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

fn windows_directory() -> Option<String> {
    let mut buffer = vec![0u16; 32768];
    let length = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if length == 0 || length as usize >= buffer.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn wait_for_restart_delay(
    service_name: &str,
    delay: u32,
    throttle_ms: u32,
    restart_delay_ms: u32,
    throttle: &mut nssm_rs::RestartThrottle,
) {
    let throttle_count = throttle.counter();
    HOOK_THROTTLE_COUNT.store(throttle_count, Ordering::SeqCst);
    if delay == 0 {
        return;
    }
    let backoff_ms = nssm_rs::restart_wait_ms(throttle_count.saturating_sub(1), 0);
    if restart_delay_ms > backoff_ms {
        report_event(
            windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
            EVENT_RESTART_DELAY,
            &[service_name, &delay.to_string()],
        );
    } else {
        report_event(
            windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE,
            EVENT_THROTTLED,
            &[service_name, &throttle_ms.to_string(), &delay.to_string()],
        );
    }
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
            Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

fn retry_failed_restart_launch(
    service_name: &str,
    app: &str,
    throttle_ms: u32,
    restart_delay_ms: u32,
    throttle: &mut nssm_rs::RestartThrottle,
) {
    report_event(
        windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE,
        EVENT_RESTART_SERVICE_FAILED,
        &[app, service_name],
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !STOP_REQUESTED.load(Ordering::SeqCst) && Instant::now() < deadline {
        thread::sleep(
            Duration::from_millis(250).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    if !STOP_REQUESTED.load(Ordering::SeqCst) {
        let delay = throttle.after_exit();
        wait_for_restart_delay(service_name, delay, throttle_ms, restart_delay_ms, throttle);
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
        let error = io::Error::last_os_error();
        if is_administrator() {
            report_event(
                windows_sys::Win32::System::EventLog::EVENTLOG_ERROR_TYPE,
                EVENT_OPENSCMANAGER_FAILED,
                &[],
            );
        }
        Err(error.to_string())
    } else {
        Ok(ScHandle(h))
    }
}

fn open_service(name: &str, access: u32) -> Result<ScHandle, String> {
    let canonical_name = canonical_service_name(name).map_err(|error| error.to_string())?;
    let manager = open_manager(SC_MANAGER_CONNECT)?;
    let name = wide(OsStr::new(&canonical_name));
    let h = unsafe { OpenServiceW(manager.0, name.as_ptr(), access) };
    if h.is_null() {
        Err(io::Error::last_os_error().to_string())
    } else {
        Ok(ScHandle(h))
    }
}

fn enable_recovery_for_non_crash_failures(service: SC_HANDLE, name: &str) {
    let flag = SERVICE_FAILURE_ACTIONS_FLAG {
        fFailureActionsOnNonCrashFailures: 1,
    };
    if unsafe {
        ChangeServiceConfig2W(
            service,
            SERVICE_CONFIG_FAILURE_ACTIONS_FLAG,
            (&flag as *const SERVICE_FAILURE_ACTIONS_FLAG).cast(),
        )
    } == 0
    {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(124) {
            let message = windows_error_message(&error);
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_SERVICE_CONFIG_FAILURE_ACTIONS_FAILED,
                &[name, &message],
            );
        }
    }
}

fn canonical_service_name(name: &str) -> Result<String, io::Error> {
    let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(io::Error::last_os_error());
    }
    let service_name = wide(OsStr::new(name));
    let service = unsafe { OpenServiceW(manager, service_name.as_ptr(), SERVICE_QUERY_STATUS) };
    if !service.is_null() {
        unsafe {
            CloseServiceHandle(service);
            CloseServiceHandle(manager);
        }
        return Ok(name.to_owned());
    }
    let open_error = io::Error::last_os_error();
    unsafe { CloseServiceHandle(manager) };
    if open_error.raw_os_error() != Some(1060) {
        return Ok(name.to_owned());
    }

    let manager = unsafe {
        OpenSCManagerW(
            ptr::null(),
            ptr::null(),
            SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE,
        )
    };
    if manager.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut required = 0;
    let mut returned = 0;
    let mut resume = 0;
    unsafe {
        EnumServicesStatusExW(
            manager,
            SC_ENUM_PROCESS_INFO,
            SERVICE_DRIVER | SERVICE_FILE_SYSTEM_DRIVER | SERVICE_KERNEL_DRIVER | SERVICE_WIN32,
            SERVICE_STATE_ALL,
            ptr::null_mut(),
            0,
            &mut required,
            &mut returned,
            &mut resume,
            ptr::null(),
        );
    }
    let enumeration_error = io::Error::last_os_error();
    if enumeration_error.raw_os_error() != Some(234) {
        unsafe { CloseServiceHandle(manager) };
        return Err(enumeration_error);
    }
    let mut storage = vec![0usize; (required as usize).div_ceil(std::mem::size_of::<usize>())];
    loop {
        let mut bytes_needed = 0;
        let mut count = 0;
        let success = unsafe {
            EnumServicesStatusExW(
                manager,
                SC_ENUM_PROCESS_INFO,
                SERVICE_DRIVER | SERVICE_FILE_SYSTEM_DRIVER | SERVICE_KERNEL_DRIVER | SERVICE_WIN32,
                SERVICE_STATE_ALL,
                storage.as_mut_ptr().cast(),
                (storage.len() * std::mem::size_of::<usize>()) as u32,
                &mut bytes_needed,
                &mut count,
                &mut resume,
                ptr::null(),
            )
        };
        let error = if success == 0 {
            Some(io::Error::last_os_error())
        } else {
            None
        };
        if let Some(error) = &error
            && error.raw_os_error() != Some(234)
        {
            unsafe { CloseServiceHandle(manager) };
            return Err(io::Error::from_raw_os_error(
                error.raw_os_error().unwrap_or_default(),
            ));
        }
        let entries = storage.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>();
        for index in 0..count as usize {
            let entry = unsafe { &*entries.add(index) };
            if windows_strings_equal(name, &wide_pointer(entry.lpDisplayName)) {
                let service_name = wide_pointer(entry.lpServiceName);
                unsafe { CloseServiceHandle(manager) };
                return Ok(service_name);
            }
        }
        if let Some(error) = error {
            if bytes_needed > (storage.len() * std::mem::size_of::<usize>()) as u32 {
                storage.resize(
                    (bytes_needed as usize).div_ceil(std::mem::size_of::<usize>()),
                    0,
                );
            }
            if error.raw_os_error() == Some(234) {
                continue;
            }
        } else {
            unsafe { CloseServiceHandle(manager) };
            return Err(open_error);
        }
    }
}

fn missing_service_error(name: &str) -> Option<io::Error> {
    match canonical_service_name(name) {
        Err(error) if error.raw_os_error() == Some(1060) => Some(error),
        _ => None,
    }
}

fn missing_process_service_diagnostic() -> String {
    let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        let error = io::Error::last_os_error();
        return format!(
            "Can't open service!\r\r\nEnumServicesStatus(): {}\r\r\n\r",
            windows_error_message(&error)
        );
    }
    let mut required = 0;
    let mut returned = 0;
    let mut resume = 0;
    unsafe {
        EnumServicesStatusExW(
            manager,
            SC_ENUM_PROCESS_INFO,
            SERVICE_DRIVER | SERVICE_FILE_SYSTEM_DRIVER | SERVICE_KERNEL_DRIVER | SERVICE_WIN32,
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
    unsafe { CloseServiceHandle(manager) };
    format!(
        "Can't open service!\r\r\nEnumServicesStatus(): {}\r\r\n\r",
        windows_error_message(&error)
    )
}

fn print_open_service_error(error: &io::Error) {
    let error_message = windows_error_message(error);
    cli_eprintln!("Can't open service!\r\r\nOpenService(): {error_message}\r\r\n\r\r");
}

fn is_nssm_service(name: &str) -> bool {
    if read_optional_reg_string(name, "Application")
        .ok()
        .flatten()
        .is_some()
    {
        return true;
    }
    let Ok(handle) = open_service(name, SERVICE_QUERY_CONFIG) else {
        return false;
    };
    let Ok((storage, config)) = query_service_config(handle.0) else {
        return false;
    };
    let _keep_storage_alive = storage;
    let image_path = wide_pointer(config.lpBinaryPathName);
    let Some(executable) = split_windows_args(&image_path).first().cloned() else {
        return false;
    };
    Path::new(&executable)
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            name == "nssm.exe" || (name.starts_with("nssm-rs") && name.ends_with(".exe"))
        })
}

fn known_setting(parameter: &str) -> bool {
    managed_setting(parameter).is_some()
        || NATIVE_SETTING_NAMES
            .iter()
            .any(|name| parameter.eq_ignore_ascii_case(name))
}

fn print_invalid_parameter(parameter: &str) {
    cli_eprintln!("Invalid parameter \"{parameter}\".  Valid parameters are:\r\r");
    for setting in MANAGED_SETTINGS {
        cli_eprintln!("{}\r", setting.name);
    }
    for setting in NATIVE_SETTING_NAMES {
        cli_eprintln!("{setting}\r");
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
    let name = canonical_service_name(name).map_err(|error| error.to_string())?;
    let name = name.as_str();
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
                cli_println!("{name}\r");
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
}

struct ProcessImage {
    path: String,
    creation_time: u64,
}

enum ProcessImageError {
    Open(io::Error),
    Times,
}

#[cfg(test)]
static FAIL_NEXT_PROCESS_SNAPSHOT: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static FAIL_NEXT_PROCESS_ENUMERATION: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static FAIL_OPEN_PROCESS_PID: AtomicU32 = AtomicU32::new(0);
#[cfg(test)]
static FAIL_NEXT_PROCESS_TIMES: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static FAIL_NEXT_STDIO_DUPLICATE: AtomicBool = AtomicBool::new(false);

fn process_snapshot() -> io::Result<HANDLE> {
    #[cfg(test)]
    if FAIL_NEXT_PROCESS_SNAPSHOT.swap(false, Ordering::SeqCst) {
        return Err(io::Error::from_raw_os_error(5));
    }

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(snapshot)
    }
}

fn first_process_entry(snapshot: HANDLE, entry: *mut PROCESSENTRY32W) -> i32 {
    #[cfg(test)]
    if FAIL_NEXT_PROCESS_ENUMERATION.swap(false, Ordering::SeqCst) {
        unsafe { SetLastError(5) };
        return 0;
    }
    unsafe { Process32FirstW(snapshot, entry) }
}

fn open_process(pid: u32) -> HANDLE {
    #[cfg(test)]
    if FAIL_OPEN_PROCESS_PID
        .compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe { SetLastError(5) };
        return std::ptr::null_mut();
    }
    unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) }
}

fn get_process_times(
    process: HANDLE,
    creation: &mut FILETIME,
    exit: &mut FILETIME,
    kernel: &mut FILETIME,
    user: &mut FILETIME,
) -> i32 {
    #[cfg(test)]
    if FAIL_NEXT_PROCESS_TIMES.swap(false, Ordering::SeqCst) {
        unsafe { SetLastError(5) };
        return 0;
    }
    unsafe { GetProcessTimes(process, creation, exit, kernel, user) }
}

fn filetime_value(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

fn process_image(pid: u32) -> Result<ProcessImage, ProcessImageError> {
    let process = open_process(pid);
    if process.is_null() {
        return Err(ProcessImageError::Open(io::Error::last_os_error()));
    }
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if get_process_times(process, &mut creation, &mut exit, &mut kernel, &mut user) == 0 {
        let error = io::Error::last_os_error();
        let error_message = windows_error_message(&error);
        report_event(
            EVENTLOG_ERROR_TYPE,
            EVENT_GET_PROCESS_TIMES_FAILED,
            &[&error_message],
        );
        unsafe {
            CloseHandle(process);
        }
        return Err(ProcessImageError::Times);
    }
    let mut path = vec![0u16; 32768];
    let mut size = path.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, path.as_mut_ptr(), &mut size)
    };
    unsafe {
        CloseHandle(process);
    }
    let path = if ok == 0 {
        "???".to_owned()
    } else {
        String::from_utf16_lossy(&path[..size as usize])
    };
    Ok(ProcessImage {
        path,
        creation_time: filetime_value(creation),
    })
}

fn process_belongs_to_service_lifetime(
    root_created: u64,
    root_exited: u64,
    child_created: u64,
) -> bool {
    root_created <= child_created && child_created <= root_exited
}

fn process_tree(service: &str) -> Result<(), String> {
    let canonical = canonical_service_name(service).map_err(|error| error.to_string())?;
    let service = canonical.as_str();
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
    process_tree_from_root(service, root)
}

fn process_tree_from_root(service: &str, root: u32) -> Result<(), String> {
    if root == 0 {
        return Ok(());
    }
    let root_image = match process_image(root) {
        Ok(image) => image,
        Err(ProcessImageError::Open(error)) => {
            let message = process_open_error_output(service, root, &error);
            write_cli_output(&message, true);
            return Ok(());
        }
        Err(ProcessImageError::Times) => return Ok(()),
    };
    let mut service_exit_time = FILETIME::default();
    unsafe { GetSystemTimeAsFileTime(&mut service_exit_time) };
    let service_lifetime = (root_image.creation_time, filetime_value(service_exit_time));
    let snapshot = match process_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let error_message = windows_error_message(&error);
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_PROCESS_SNAPSHOT_FAILED,
                &[service, &error_message],
            );
            return Err(error.to_string());
        }
    };
    let mut processes = HashMap::<u32, Vec<ProcessEntry>>::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut ok = first_process_entry(snapshot, &mut entry);
    if ok == 0 {
        let error = io::Error::last_os_error();
        let error_message = windows_error_message(&error);
        report_event(
            EVENTLOG_ERROR_TYPE,
            EVENT_PROCESS_ENUMERATE_FAILED,
            &[service, &error_message],
        );
    }
    while ok != 0 {
        processes
            .entry(entry.th32ParentProcessID)
            .or_default()
            .push(ProcessEntry {
                pid: entry.th32ProcessID,
            });
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                let error_message = windows_error_message(&error);
                report_event(
                    EVENTLOG_ERROR_TYPE,
                    EVENT_PROCESS_ENUMERATE_FAILED,
                    &[service, &error_message],
                );
            }
        }
    }
    unsafe {
        CloseHandle(snapshot);
    }
    fn visit(
        service: &str,
        pid: u32,
        depth: usize,
        root_image: Option<&ProcessImage>,
        service_lifetime: (u64, u64),
        tree: &HashMap<u32, Vec<ProcessEntry>>,
        seen: &mut HashSet<u32>,
    ) {
        if !seen.insert(pid) {
            return;
        }
        let image = if depth == 0 {
            root_image.map(|image| ProcessImage {
                path: image.path.clone(),
                creation_time: image.creation_time,
            })
        } else {
            match process_image(pid) {
                Ok(image) => Some(image),
                Err(ProcessImageError::Open(error)) => {
                    let pid = pid.to_string();
                    let error_message = windows_error_message(&error);
                    report_event(
                        EVENTLOG_ERROR_TYPE,
                        EVENT_OPEN_PROCESS_FAILED,
                        &[&pid, service, &error_message],
                    );
                    return;
                }
                Err(ProcessImageError::Times) => return,
            }
        };
        let Some(image) = image else { return };
        if !process_belongs_to_service_lifetime(
            service_lifetime.0,
            service_lifetime.1,
            image.creation_time,
        ) {
            return;
        }
        cli_println!("{pid:>8} {}{}\r", " ".repeat(depth), image.path);
        if let Some(children) = tree.get(&pid) {
            for child in children {
                visit(
                    service,
                    child.pid,
                    depth + 1,
                    None,
                    service_lifetime,
                    tree,
                    seen,
                );
            }
        }
    }
    visit(
        service,
        root,
        0,
        Some(&root_image),
        service_lifetime,
        &processes,
        &mut HashSet::new(),
    );
    Ok(())
}

fn processes(services: &[String]) -> Result<(), String> {
    let mut failed = false;
    for service in services {
        if missing_service_error(service).is_some() {
            let diagnostic = missing_process_service_diagnostic();
            write_cli_output(&format!("{diagnostic}\r\n"), true);
            failed = true;
            continue;
        }
        if let Err(error) = process_tree(service) {
            write_cli_output(&format!("{service}: {error}\r\n"), true);
            failed = true;
        }
    }
    if failed { Err(String::new()) } else { Ok(()) }
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
    let result = write_reg_string(key, "Application", app)
        .and_then(|_| {
            let joined = args.join(" ");
            write_reg_string(key, "AppParameters", &joined)
        })
        .and_then(|_| {
            let directory = Path::new(app)
                .parent()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default();
            write_reg_string(key, "AppDirectory", &directory)
        });
    unsafe {
        RegCloseKey(key);
    }
    result?;
    let exit_key = open_registry_key(name, "AppExit", true, KEY_SET_VALUE)?;
    write_raw_value(exit_key.0, None, REG_SZ, &encode_wide("Restart", 1))
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
            SERVICE_QUERY_STATUS
                | SERVICE_START
                | SERVICE_STOP
                | SERVICE_CHANGE_CONFIG
                | DELETE_ACCESS,
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
    enable_recovery_for_non_crash_failures(service.0, name);
    cli_println!("Installed service '{name}'.\r");
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
    if code == 2 {
        return Ok(None);
    }
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
    let mut data = vec![0u16; (bytes as usize).div_ceil(2)];
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

fn read_stop_method_skip(service: &str) -> u32 {
    let key = match open_registry_key(service, "", false, KEY_QUERY_VALUE) {
        Ok(key) => key,
        Err(error) => {
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_QUERYVALUE_FAILED,
                &["AppStopMethodSkip", &error],
            );
            return 0;
        }
    };
    let value_name = wide(OsStr::new("AppStopMethodSkip"));
    let mut kind = 0;
    let mut data = 0u32;
    let mut bytes = std::mem::size_of::<u32>() as u32;
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            value_name.as_ptr(),
            ptr::null(),
            &mut kind,
            (&mut data as *mut u32).cast(),
            &mut bytes,
        )
    };
    if code == 2 {
        return 0;
    }
    if kind != REG_DWORD {
        report_event(
            windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE,
            EVENT_BOGUS_STOP_METHOD_SKIP,
            &[service, "AppStopMethodSkip", "NSSM"],
        );
        return 0;
    }
    if code != 0 {
        let error = registry_error(code);
        report_event(
            EVENTLOG_ERROR_TYPE,
            EVENT_QUERYVALUE_FAILED,
            &["AppStopMethodSkip", &error],
        );
        return 0;
    }
    if bytes != std::mem::size_of::<u32>() as u32 {
        report_event(
            windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE,
            EVENT_BOGUS_STOP_METHOD_SKIP,
            &[service, "AppStopMethodSkip", "NSSM"],
        );
        return 0;
    }
    data
}

#[derive(Debug)]
enum StdioFileError {
    Configuration,
    Create {
        path: String,
        error: io::Error,
    },
    Duplicate {
        source: &'static str,
        destination: &'static str,
        error: io::Error,
    },
    Seek,
}

fn duplicate_stdio_file(file: &File) -> io::Result<File> {
    #[cfg(test)]
    if FAIL_NEXT_STDIO_DUPLICATE.swap(false, Ordering::SeqCst) {
        return Err(io::Error::from_raw_os_error(5));
    }
    file.try_clone()
}

impl From<String> for StdioFileError {
    fn from(_: String) -> Self {
        Self::Configuration
    }
}

fn open_stdio_file(
    service: &str,
    setting: &str,
    child_environment: &[String],
    input: bool,
) -> Result<Option<File>, StdioFileError> {
    let Some(path) = read_optional_reg_string(service, setting)? else {
        return Ok(None);
    };
    let path = expand_environment_value(&path, child_environment);
    let prefix = setting.to_owned();
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
            return Err(format!("invalid {setting} creation disposition: {disposition}").into());
        }
    }
    let mut file = options
        .open(&path)
        .map_err(|error| StdioFileError::Create {
            path: path.clone(),
            error,
        })?;
    if !input {
        file.seek(SeekFrom::End(0))
            .map_err(|_| StdioFileError::Seek)?;
    }
    Ok(Some(file))
}

fn report_stdio_setup_failure(service: &str, app: &str, error: &StdioFileError) {
    match error {
        StdioFileError::Create { path, error } => {
            let error_message = windows_error_message(error);
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_CREATEFILE_FAILED,
                &[path, &error_message],
            );
        }
        StdioFileError::Duplicate {
            source,
            destination,
            error,
        } => {
            let error_message = windows_error_message(error);
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_DUPLICATEHANDLE_FAILED,
                &[source, destination, &error_message],
            );
        }
        StdioFileError::Configuration | StdioFileError::Seek => {}
    }
    report_event(
        EVENTLOG_ERROR_TYPE,
        EVENT_GET_OUTPUT_HANDLES_FAILED,
        &[service],
    );
    report_event(
        EVENTLOG_ERROR_TYPE,
        EVENT_START_SERVICE_FAILED,
        &[app, service, "4"],
    );
}

fn windows_strings_equal(first: &str, second: &str) -> bool {
    let first: Vec<u16> = first.encode_utf16().collect();
    let second: Vec<u16> = second.encode_utf16().collect();
    unsafe {
        CompareStringOrdinal(
            first.as_ptr(),
            first.len() as i32,
            second.as_ptr(),
            second.len() as i32,
            1,
        ) == CSTR_EQUAL
    }
}

fn windows_error_message(error: &io::Error) -> String {
    let Some(code) = error.raw_os_error() else {
        return error.to_string();
    };
    let mut buffer = vec![0u16; 2048];
    let flags = windows_sys::Win32::System::Diagnostics::Debug::FORMAT_MESSAGE_FROM_SYSTEM
        | windows_sys::Win32::System::Diagnostics::Debug::FORMAT_MESSAGE_IGNORE_INSERTS;
    let length = unsafe {
        FormatMessageW(
            flags,
            ptr::null(),
            code as u32,
            GetUserDefaultUILanguage() as u32,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            ptr::null(),
        )
    };
    let length = if length == 0 {
        unsafe {
            FormatMessageW(
                flags,
                ptr::null(),
                code as u32,
                0,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                ptr::null(),
            )
        }
    } else {
        length
    };
    if length == 0 {
        return error.to_string();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

fn process_open_error_output(service: &str, pid: u32, error: &io::Error) -> String {
    format!(
        "{service}: {pid}: {}\r\r\n\r\n",
        windows_error_message(error)
    )
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
) -> Result<Option<std::path::PathBuf>, RotationFailure> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RotationFailure::from_io("CreateFile()", path, &error)),
    };
    let modified = metadata
        .modified()
        .map_err(|error| RotationFailure::from_io("GetFileInformationByHandle()", path, &error))?;
    let age = modified.elapsed().unwrap_or_default();
    if !log_rotation_due(metadata.len(), age, minimum_age, minimum_size, force) {
        return Ok(None);
    }
    let rotated = rotated_log_path(Path::new(path), modified).ok_or_else(|| {
        RotationFailure::from_message(
            "rotated_filename()",
            path,
            "could not create rotated filename",
        )
    })?;
    if copy_and_truncate || live_handle {
        fs::copy(path, &rotated).map_err(|error| {
            RotationFailure::from_io("CopyFile()", &rotated.to_string_lossy(), &error)
        })?;
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        let file = OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|error| RotationFailure::from_io("CreateFile()", path, &error))?;
        file.set_len(0)
            .map_err(|error| RotationFailure::from_io("SetEndOfFile()", path, &error))?;
    } else {
        fs::rename(path, &rotated).map_err(|error| {
            RotationFailure::from_io("MoveFile()", &rotated.to_string_lossy(), &error)
        })?;
    }
    Ok(Some(rotated))
}

fn report_rotation_event(service: &str, path: &str, rotated: &std::path::Path) {
    report_event(
        windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
        EVENT_ROTATED,
        &[service, path, &rotated.to_string_lossy()],
    );
}

fn report_rotation_failure(service: &str, path: &str, error: &RotationFailure) {
    report_event(
        EVENTLOG_ERROR_TYPE,
        EVENT_ROTATE_FILE_FAILED,
        &[
            service,
            path,
            error.operation,
            &error.target,
            &error.message,
        ],
    );
}

unsafe extern "system" fn post_close_to_process_window(hwnd: HWND, target_pid: LPARAM) -> i32 {
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) };
    if process_id == target_pid as u32 {
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
            PostMessageW(
                hwnd,
                WM_ENDSESSION,
                1,
                (ENDSESSION_CLOSEAPP | ENDSESSION_CRITICAL | ENDSESSION_LOGOFF) as isize,
            );
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

fn update_stop_pending_progress(interval_ms: u32) {
    let wait_hint = STOP_WAIT_HINT
        .fetch_add(interval_ms, Ordering::SeqCst)
        .saturating_add(interval_ms);
    let checkpoint = STOP_WAIT_CHECKPOINT.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = SERVICE_STATUS_HANDLE_VALUE.load(Ordering::SeqCst) as SERVICE_STATUS_HANDLE;
    if handle.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: SERVICE_STOP_PENDING,
        dwControlsAccepted: 0,
        dwWin32ExitCode: NO_ERROR,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: checkpoint,
        dwWaitHint: wait_hint,
    };
    unsafe { SetServiceStatus(handle, &status) };
}

fn wait_process_for(child: &mut Child, grace: Duration, service: &str, function: &str) -> bool {
    const STATUS_DEADLINE_MS: u32 = 20_000;
    let timeout_ms = grace.as_millis().min(u32::MAX as u128) as u32;
    let mut waited_ms = 0;
    while waited_ms < timeout_ms {
        let interval_ms = (timeout_ms - waited_ms).min(STATUS_DEADLINE_MS);
        update_stop_pending_progress(interval_ms);
        if waited_ms != 0 {
            report_event(
                EVENTLOG_INFORMATION_TYPE,
                EVENT_AWAITING_SINGLE_HANDLE,
                &[
                    function,
                    service,
                    &waited_ms.to_string(),
                    &interval_ms.to_string(),
                    &timeout_ms.to_string(),
                ],
            );
        }
        let interval_deadline = Instant::now() + Duration::from_millis(interval_ms as u64);
        loop {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => return true,
                Ok(None) if Instant::now() >= interval_deadline => break,
                Ok(None) => thread::sleep(Duration::from_millis(25)),
            }
        }
        waited_ms += interval_ms;
    }
    child.try_wait().map_or(true, |status| status.is_some())
}

fn report_process_tree_events(service: &str, root: u32, exit_code: u32, report_still_active: bool) {
    let snapshot = match process_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let message = windows_error_message(&error);
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_PROCESS_SNAPSHOT_FAILED,
                &[service, &message],
            );
            return;
        }
    };

    let mut children = HashMap::<u32, Vec<u32>>::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut ok = first_process_entry(snapshot, &mut entry);
    if ok == 0 {
        let error = io::Error::last_os_error();
        let message = windows_error_message(&error);
        report_event(
            EVENTLOG_ERROR_TYPE,
            EVENT_PROCESS_ENUMERATE_FAILED,
            &[service, &message],
        );
    }
    while ok != 0 {
        children
            .entry(entry.th32ParentProcessID)
            .or_default()
            .push(entry.th32ProcessID);
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                let message = windows_error_message(&error);
                report_event(
                    EVENTLOG_ERROR_TYPE,
                    EVENT_PROCESS_ENUMERATE_FAILED,
                    &[service, &message],
                );
            }
        }
    }
    unsafe { CloseHandle(snapshot) };

    fn visit(
        service: &str,
        pid: u32,
        parent: u32,
        exit_code: u32,
        report_still_active: bool,
        children: &HashMap<u32, Vec<u32>>,
        seen: &mut HashSet<u32>,
    ) {
        if !seen.insert(pid) {
            return;
        }
        let pid_text = pid.to_string();
        let exit_text = exit_code.to_string();
        report_event(
            EVENTLOG_INFORMATION_TYPE,
            EVENT_KILLING,
            &[service, &pid_text, &exit_text],
        );
        let process = open_process(pid);
        if process.is_null() {
            let error = io::Error::last_os_error();
            let message = windows_error_message(&error);
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_OPEN_PROCESS_FAILED,
                &[&pid_text, service, &message],
            );
        } else {
            let parent_text = parent.to_string();
            report_event(
                EVENTLOG_INFORMATION_TYPE,
                EVENT_KILL_PROCESS_TREE,
                &[&pid_text, &parent_text, service],
            );
            if report_still_active {
                let mut process_exit_code = 0;
                if unsafe { GetExitCodeProcess(process, &mut process_exit_code) } == 0
                    || process_exit_code == windows_sys::Win32::Foundation::STILL_ACTIVE as u32
                {
                    report_event(
                        windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE,
                        EVENT_PROCESS_STILL_ACTIVE,
                        &[service, &pid_text, "NSSM", "AppStopMethodSkip"],
                    );
                }
            }
            unsafe { CloseHandle(process) };
        }
        if let Some(descendants) = children.get(&pid) {
            for descendant in descendants {
                visit(
                    service,
                    *descendant,
                    pid,
                    exit_code,
                    report_still_active,
                    children,
                    seen,
                );
            }
        }
    }

    let mut seen = HashSet::new();
    visit(
        service,
        root,
        std::process::id(),
        exit_code,
        report_still_active,
        &children,
        &mut seen,
    );
}

enum StopChildOutcome {
    Exited { tree_events_logged: bool },
    StillActive,
}

fn stop_child(
    service: &str,
    application: &str,
    child: &mut ManagedChild,
    no_console: bool,
    skip: u32,
) -> Result<StopChildOutcome, String> {
    report_event(
        windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
        EVENT_TERMINATEPROCESS,
        &[service, application],
    );
    let pid = child.child.id();
    let mut tree_events_logged = false;
    for method in nssm_rs::stop_methods(skip, no_console) {
        match method {
            StopMethod::Console | StopMethod::Window | StopMethod::Threads => {
                let (setting, signal, function) = match method {
                    StopMethod::Console => (
                        "AppStopMethodConsole",
                        send_console_ctrl_c as fn(u32),
                        "kill_console()",
                    ),
                    StopMethod::Window => (
                        "AppStopMethodWindow",
                        send_window_close as fn(u32),
                        "kill_process()",
                    ),
                    StopMethod::Threads => (
                        "AppStopMethodThreads",
                        send_thread_quit as fn(u32),
                        "kill_process()",
                    ),
                    StopMethod::Terminate => unreachable!(),
                };
                signal(pid);
                let delay = read_reg_dword(service, setting, 1500)?;
                if wait_process_for(
                    &mut child.child,
                    Duration::from_millis(delay as u64),
                    service,
                    function,
                ) {
                    return Ok(StopChildOutcome::Exited { tree_events_logged });
                }
            }
            StopMethod::Terminate => {
                if child.job != 0 {
                    report_process_tree_events(service, pid, 0, false);
                    tree_events_logged = true;
                    if unsafe { TerminateJobObject(child.job as HANDLE, 0) } == 0 {
                        unsafe {
                            TerminateProcess(child.child.as_raw_handle() as HANDLE, 0);
                        }
                    }
                } else {
                    unsafe { TerminateProcess(child.child.as_raw_handle() as HANDLE, 0) };
                }
            }
        }
    }
    if skip & STOP_SKIP_TERMINATE != 0
        && child.child.try_wait().map_err(|e| e.to_string())?.is_none()
    {
        return Ok(StopChildOutcome::StillActive);
    }
    while child.child.try_wait().map_err(|e| e.to_string())?.is_none() {
        thread::sleep(Duration::from_millis(100));
    }
    Ok(StopChildOutcome::Exited { tree_events_logged })
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
    let mut data = vec![0u16; (bytes as usize).div_ceil(2)];
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

fn hook_trigger(event: &str, control: u32) -> &'static str {
    match event {
        "Start" => control_name(0),
        "Stop" | "Rotate" | "Power" => control_name(control),
        _ => "",
    }
}

fn hook_exit_code(application_pid: Option<u32>, last_exit_code: u32) -> Option<u32> {
    application_pid.is_none().then_some(last_exit_code)
}

struct HookAbiContext<'a> {
    event: &'a str,
    action: &'a str,
    trigger: &'a str,
    configuration: &'a str,
    build_date: &'a str,
    service: &'a str,
    display_name: &'a str,
    executable: &'a str,
    version: &'a str,
    pid: u32,
    last_control: &'a str,
    start_requested_count: u32,
    start_count: u32,
    throttle_count: u32,
    exit_count: u32,
    runtime: &'a str,
    application_runtime: &'a str,
    application_pid: Option<u32>,
    exit_code: Option<u32>,
    command_line: &'a str,
    deadline: u64,
}

fn hook_abi_environment(context: HookAbiContext<'_>) -> Vec<(&'static str, String)> {
    vec![
        ("NSSM_HOOK_VERSION", "1".to_owned()),
        ("NSSM_EVENT", context.event.to_owned()),
        ("NSSM_ACTION", context.action.to_owned()),
        ("NSSM_TRIGGER", context.trigger.to_owned()),
        ("NSSM_CONFIGURATION", context.configuration.to_owned()),
        ("NSSM_BUILD_DATE", context.build_date.to_owned()),
        ("NSSM_SERVICE_NAME", context.service.to_owned()),
        ("NSSM_SERVICE_DISPLAYNAME", context.display_name.to_owned()),
        ("NSSM_EXE", context.executable.to_owned()),
        ("NSSM_VERSION", context.version.to_owned()),
        ("NSSM_PID", context.pid.to_string()),
        ("NSSM_LAST_CONTROL", context.last_control.to_owned()),
        (
            "NSSM_START_REQUESTED_COUNT",
            context.start_requested_count.to_string(),
        ),
        ("NSSM_START_COUNT", context.start_count.to_string()),
        ("NSSM_THROTTLE_COUNT", context.throttle_count.to_string()),
        ("NSSM_EXIT_COUNT", context.exit_count.to_string()),
        ("NSSM_RUNTIME", context.runtime.to_owned()),
        (
            "NSSM_APPLICATION_RUNTIME",
            context.application_runtime.to_owned(),
        ),
        (
            "NSSM_APPLICATION_PID",
            context
                .application_pid
                .map_or_else(String::new, |pid| pid.to_string()),
        ),
        (
            "NSSM_EXITCODE",
            context
                .exit_code
                .map_or_else(String::new, |code| code.to_string()),
        ),
        ("NSSM_COMMAND_LINE", context.command_line.to_owned()),
        ("NSSM_DEADLINE", context.deadline.to_string()),
    ]
}

fn service_control_event(control: u32, handled: bool) -> (u32, String) {
    let name = control_name(control);
    if name.is_empty() {
        (EVENT_SERVICE_CONTROL_UNKNOWN, format!("0x{control:08x}"))
    } else if handled {
        (EVENT_SERVICE_CONTROL_HANDLED, name.to_owned())
    } else {
        (EVENT_SERVICE_CONTROL_NOT_HANDLED, name.to_owned())
    }
}

fn report_service_control(service: &str, control: u32, handled: bool) {
    let (event, text) = service_control_event(control, handled);
    report_event(
        windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
        event,
        &[service, &text],
    );
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

// Each argument is an independent part of NSSM's hook ABI and wait policy.
#[expect(clippy::too_many_arguments)]
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
    let command_line = match read_hook(service, &format!("{event}/{action}")) {
        Ok(command_line) => command_line,
        Err(error) => {
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_GET_HOOK_FAILED,
                &[event, action, service],
            );
            return Err(error);
        }
    };
    let Some(command_line) = command_line else {
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
    let image = std::env::current_exe()
        .map_err(|error| error.to_string())?
        .to_string_lossy()
        .into_owned();
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
    let last_control = HOOK_LAST_CONTROL.load(Ordering::SeqCst);
    let trigger = hook_trigger(event, last_control);
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
    let hook_context = HookAbiContext {
        event,
        action,
        trigger,
        configuration: &configuration,
        build_date: env!("NSSM_BUILD_DATE"),
        service,
        display_name: &display_name,
        executable: &image,
        version: nssm_rs::VERSION,
        pid: std::process::id(),
        last_control: control_name(last_control),
        start_requested_count: HOOK_START_REQUESTED_COUNT.load(Ordering::SeqCst),
        start_count: HOOK_START_COUNT.load(Ordering::SeqCst),
        throttle_count: HOOK_THROTTLE_COUNT.load(Ordering::SeqCst),
        exit_count: HOOK_EXIT_COUNT.load(Ordering::SeqCst),
        runtime: &service_runtime,
        application_runtime: &application_runtime,
        application_pid: app_pid,
        exit_code,
        command_line: &application_command_line,
        deadline: deadline_ms,
    };
    command.envs(hook_abi_environment(hook_context));
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
    let mut hook = match command.spawn() {
        Ok(hook) => hook,
        Err(error) => {
            let error_message = windows_error_message(&error);
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_HOOK_CREATEPROCESS_FAILED,
                &[event, action, service, &command_line, &error_message],
            );
            return Err(format!("unable to launch {event}/{action} hook: {error}"));
        }
    };
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
    context: *mut std::ffi::c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            HOOK_LAST_CONTROL.store(control, Ordering::SeqCst);
            report_service_control(&wide_pointer(context.cast()), control, true);
            STOP_WAIT_CHECKPOINT.store(0, Ordering::SeqCst);
            STOP_WAIT_HINT.store(10_000, Ordering::SeqCst);
            STOP_REQUESTED.store(true, Ordering::SeqCst);
            set_status(SERVICE_STOP_PENDING, 0, 10_000);
        }
        128 => {
            if !ROTATE_ENABLED.load(Ordering::SeqCst) {
                report_service_control(&wide_pointer(context.cast()), control, false);
                return 120;
            }
            HOOK_LAST_CONTROL.store(control, Ordering::SeqCst);
            report_service_control(&wide_pointer(context.cast()), control, true);
            ROTATE_REQUESTED.store(true, Ordering::SeqCst);
        }
        SERVICE_CONTROL_POWEREVENT => {
            let Some(invocation) = power_hook_invocation(event_type) else {
                report_service_control(&wide_pointer(context.cast()), control, false);
                return NO_ERROR;
            };
            HOOK_LAST_CONTROL.store(invocation.control, Ordering::SeqCst);
            let service_name = wide_pointer(context.cast());
            report_service_control(&service_name, invocation.control, true);
            let child_environment = SERVICE_CHILD_ENVIRONMENT
                .lock()
                .map(|environment| environment.clone())
                .unwrap_or_default();
            let app_pid = CHILD_PROCESS
                .lock()
                .ok()
                .and_then(|child| child.as_ref().map(|child| child.child.id()));
            let exit_code = hook_exit_code(
                app_pid,
                HOOK_LAST_APPLICATION_EXIT_CODE.load(Ordering::SeqCst),
            );
            let _ = invoke_hook(
                &service_name,
                "Power",
                invocation.action.hook_action(),
                &child_environment,
                app_pid,
                exit_code,
                invocation.synchronous,
                invocation.cancel_on_stop,
            );
        }
        SERVICE_CONTROL_CONTINUE => {
            HOOK_LAST_CONTROL.store(control, Ordering::SeqCst);
            report_service_control(&wide_pointer(context.cast()), control, true);
            HOOK_THROTTLE_COUNT.store(0, Ordering::SeqCst);
            CONTINUE_REQUESTED.store(true, Ordering::SeqCst);
            let service = wide_pointer(context.cast());
            report_event(
                windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
                EVENT_RESET_THROTTLE,
                &[&service],
            );
            set_status(SERVICE_CONTINUE_PENDING, 0, 10_000);
        }
        SERVICE_CONTROL_PAUSE => {
            report_service_control(&wide_pointer(context.cast()), control, false);
            return 120;
        }
        SERVICE_CONTROL_INTERROGATE => {}
        _ => {
            report_service_control(&wide_pointer(context.cast()), control, false);
            return 120;
        }
    }
    NO_ERROR
}

struct ServiceConfig {
    app: String,
    args: Vec<String>,
    directory: String,
    app_environment: Option<Vec<String>>,
    extra_environment: Vec<String>,
    child_environment: Vec<String>,
    affinity: Option<String>,
    priority: u32,
    stop_method_skip: u32,
    no_console: bool,
    kill_process_tree: bool,
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
    CONTINUE_REQUESTED.store(false, Ordering::SeqCst);
    HOOK_START_REQUESTED_COUNT.store(0, Ordering::SeqCst);
    HOOK_START_COUNT.store(0, Ordering::SeqCst);
    HOOK_EXIT_COUNT.store(0, Ordering::SeqCst);
    HOOK_THROTTLE_COUNT.store(0, Ordering::SeqCst);
    HOOK_LAST_CONTROL.store(0, Ordering::SeqCst);
    HOOK_SERVICE_START_MS.store(unix_time_millis(), Ordering::SeqCst);
    HOOK_APPLICATION_START_MS.store(0, Ordering::SeqCst);
    HOOK_APPLICATION_RUNTIME_MS.store(0, Ordering::SeqCst);
    HOOK_LAST_APPLICATION_EXIT_CODE.store(0, Ordering::SeqCst);
    let name_wide = wide(OsStr::new(&service_name));
    let handle = unsafe {
        RegisterServiceCtrlHandlerExW(
            name_wide.as_ptr(),
            Some(service_handler),
            name_wide.as_ptr().cast_mut().cast(),
        )
    };
    if handle.is_null() {
        let error_message = windows_error_message(&io::Error::last_os_error());
        report_event(
            EVENTLOG_ERROR_TYPE,
            EVENT_REGISTER_HANDLER_FAILED,
            &[&error_message],
        );
        return;
    }
    SERVICE_STATUS_HANDLE_VALUE.store(handle as isize, Ordering::SeqCst);
    report_service_control(&service_name, 0, true);
    if is_administrator()
        && let Ok(service) = open_service(&service_name, SERVICE_CHANGE_CONFIG)
    {
        enable_recovery_for_non_crash_failures(service.0, &service_name);
    }
    set_status(SERVICE_START_PENDING, 0, 10_000);
    let _ = ensure_default_exit_action(&service_name);
    let config = (|| -> Result<ServiceConfig, String> {
        let inherited: Vec<String> = std::env::vars_os()
            .map(|(key, value)| format!("{}={}", key.to_string_lossy(), value.to_string_lossy()))
            .collect();
        let app_environment = read_multi_parameter(&service_name, "AppEnvironment")?;
        let extra_environment =
            read_multi_parameter(&service_name, "AppEnvironmentExtra")?.unwrap_or_default();
        let child_environment =
            compose_child_environment(&inherited, app_environment.as_deref(), &extra_environment)
                .map_err(|error| error.to_string())?;
        let app = expand_environment_value(
            &read_reg_string(&service_name, "Application")?,
            &child_environment,
        );
        let args = match read_optional_reg_string(&service_name, "AppParameters") {
            Ok(Some(value)) => {
                split_windows_args(&expand_environment_value(&value, &child_environment))
            }
            Ok(None) | Err(_) => {
                report_event(
                    windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE,
                    EVENT_NO_FLAGS,
                    &["AppParameters", &service_name, &app],
                );
                Vec::new()
            }
        };
        let configured_directory = read_optional_reg_string(&service_name, "AppDirectory")
            .ok()
            .flatten()
            .map(|value| expand_environment_value(&value, &child_environment))
            .filter(|value| !value.is_empty());
        let directory = match configured_directory {
            Some(value) => value,
            None => {
                let fallback = Path::new(&app)
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .map(|path| path.to_string_lossy().into_owned())
                    .or_else(windows_directory);
                let Some(value) = fallback else {
                    report_event(
                        EVENTLOG_ERROR_TYPE,
                        EVENT_DIRECTORY_FAILED,
                        &["AppDirectory", &service_name],
                    );
                    report_event(
                        EVENTLOG_ERROR_TYPE,
                        EVENT_GET_PARAMETERS_FAILED,
                        &[&service_name],
                    );
                    return Err("could not resolve AppDirectory".to_owned());
                };
                report_event(
                    windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE,
                    EVENT_NO_DIRECTORY,
                    &["AppDirectory", &service_name, &value],
                );
                value
            }
        };
        let configured_affinity = read_optional_reg_string(&service_name, "AppAffinity")?;
        let affinity = match configured_affinity {
            Some(value) if !value.is_empty() => match parse_cpu_affinity(&value) {
                Ok(mask) => {
                    if mask != 0 {
                        let (mut process_mask, mut system_mask) = (0usize, 0usize);
                        if unsafe {
                            GetProcessAffinityMask(
                                GetCurrentProcess(),
                                &mut process_mask,
                                &mut system_mask,
                            )
                        } != 0
                        {
                            let effective_mask = mask as usize & system_mask;
                            if effective_mask != mask as usize {
                                let system = format_cpu_affinity_mask(system_mask as u64)
                                    .unwrap_or_default();
                                let effective = format_cpu_affinity_mask(effective_mask as u64)
                                    .unwrap_or_default();
                                report_event(
                                    windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE,
                                    EVENT_EFFECTIVE_AFFINITY_MASK,
                                    &[&service_name, &value, &system, &effective],
                                );
                            }
                        }
                    }
                    Some(value)
                }
                Err(_) => {
                    report_event(
                        windows_sys::Win32::System::EventLog::EVENTLOG_WARNING_TYPE,
                        EVENT_BOGUS_AFFINITY_MASK,
                        &[&service_name, &value],
                    );
                    None
                }
            },
            value => value,
        };
        let configured_priority = read_reg_dword(&service_name, "AppPriority", 0x20)?;
        let priority = if valid_priority_class(configured_priority) {
            configured_priority
        } else {
            report_event(
                windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
                EVENT_BOGUS_PRIORITY,
                &[&service_name, "AppPriority"],
            );
            0x20
        };
        let stop_method_skip = read_stop_method_skip(&service_name);
        let no_console = read_reg_dword(&service_name, "AppNoConsole", 0)? != 0;
        let kill_process_tree = read_reg_dword(&service_name, "AppKillProcessTree", 1)? != 0;
        Ok(ServiceConfig {
            app,
            args,
            directory,
            app_environment,
            extra_environment,
            child_environment,
            affinity,
            priority,
            stop_method_skip,
            no_console,
            kill_process_tree,
        })
    })();
    let ServiceConfig {
        app,
        args,
        directory,
        app_environment,
        extra_environment,
        child_environment,
        affinity,
        priority,
        stop_method_skip,
        no_console,
        kill_process_tree,
    } = match config {
        Ok(config) => config,
        Err(_) => {
            set_status(SERVICE_STOPPED, 2, 0);
            return;
        }
    };
    if let Ok(mut environment) = SERVICE_CHILD_ENVIRONMENT.lock() {
        *environment = child_environment.clone();
    }
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
    let mut throttle = nssm_rs::RestartThrottle::new(restart_delay_ms);
    let mut stop_hook_invoked = false;
    let mut stop_process_handled = false;
    let mut application_started = false;
    while !STOP_REQUESTED.load(Ordering::SeqCst) {
        HOOK_START_REQUESTED_COUNT.fetch_add(1, Ordering::SeqCst);
        set_status(SERVICE_START_PENDING, 0, 10_000);
        let same_output_path = stdout_path
            .as_deref()
            .zip(stderr_path.as_deref())
            .is_some_and(|(stdout, stderr)| windows_strings_equal(stdout, stderr));
        if rotate_files {
            for (index, (path, copy)) in [
                (stdout_path.as_deref(), stdout_copy_and_truncate),
                (stderr_path.as_deref(), stderr_copy_and_truncate),
            ]
            .into_iter()
            .enumerate()
            {
                if same_output_path && index == 1 {
                    continue;
                }
                if let Some(path) = path {
                    match rotate_log_file(
                        path,
                        Duration::from_secs(rotate_seconds as u64),
                        rotate_size,
                        rotate_delay,
                        false,
                        copy,
                        false,
                    ) {
                        Ok(Some(rotated)) => report_rotation_event(&service_name, path, &rotated),
                        Ok(None) => {}
                        Err(error) => report_rotation_failure(&service_name, path, &error),
                    }
                }
            }
        }
        let redirected_stdin =
            match open_stdio_file(&service_name, "AppStdin", &child_environment, true) {
                Ok(file) => file.map(Stdio::from).unwrap_or_else(Stdio::null),
                Err(error) => {
                    report_stdio_setup_failure(&service_name, &app, &error);
                    if application_started {
                        retry_failed_restart_launch(
                            &service_name,
                            &app,
                            throttle_ms,
                            restart_delay_ms,
                            &mut throttle,
                        );
                        continue;
                    }
                    finish_async_hooks();
                    set_status_with_service_exit(
                        SERVICE_STOPPED,
                        windows_sys::Win32::Foundation::ERROR_SERVICE_SPECIFIC_ERROR,
                        4,
                        0,
                    );
                    return;
                }
            };
        let mut stdout_file =
            match open_stdio_file(&service_name, "AppStdout", &child_environment, false) {
                Ok(file) => file,
                Err(error) => {
                    report_stdio_setup_failure(&service_name, &app, &error);
                    if application_started {
                        retry_failed_restart_launch(
                            &service_name,
                            &app,
                            throttle_ms,
                            restart_delay_ms,
                            &mut throttle,
                        );
                        continue;
                    }
                    finish_async_hooks();
                    set_status_with_service_exit(
                        SERVICE_STOPPED,
                        windows_sys::Win32::Foundation::ERROR_SERVICE_SPECIFIC_ERROR,
                        4,
                        0,
                    );
                    return;
                }
            };
        let mut stderr_file = if same_output_path {
            match stdout_file.as_ref() {
                Some(file) => match duplicate_stdio_file(file) {
                    Ok(file) => Some(file),
                    Err(error) => {
                        let error = StdioFileError::Duplicate {
                            source: "stdout",
                            destination: "stderr",
                            error,
                        };
                        report_stdio_setup_failure(&service_name, &app, &error);
                        if application_started {
                            retry_failed_restart_launch(
                                &service_name,
                                &app,
                                throttle_ms,
                                restart_delay_ms,
                                &mut throttle,
                            );
                            continue;
                        }
                        finish_async_hooks();
                        set_status_with_service_exit(
                            SERVICE_STOPPED,
                            windows_sys::Win32::Foundation::ERROR_SERVICE_SPECIFIC_ERROR,
                            4,
                            0,
                        );
                        return;
                    }
                },
                None => None,
            }
        } else {
            match open_stdio_file(&service_name, "AppStderr", &child_environment, false) {
                Ok(file) => file,
                Err(error) => {
                    report_stdio_setup_failure(&service_name, &app, &error);
                    if application_started {
                        retry_failed_restart_launch(
                            &service_name,
                            &app,
                            throttle_ms,
                            restart_delay_ms,
                            &mut throttle,
                        );
                        continue;
                    }
                    finish_async_hooks();
                    set_status_with_service_exit(
                        SERVICE_STOPPED,
                        windows_sys::Win32::Foundation::ERROR_SERVICE_SPECIFIC_ERROR,
                        4,
                        0,
                    );
                    return;
                }
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
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_PRESTART_HOOK_ABORT,
                &["START", "PRE", &service_name, "99"],
            );
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
        let (stdout_destination, stderr_destination) = if (timestamp_log || rotate_online)
            && same_output_path
        {
            match (stdout_path.as_deref(), stdout_file.take()) {
                (Some(path), Some(file)) => {
                    match LogDestination::new(&service_name, "AppStdout", path.to_owned(), file) {
                        Ok(destination) => (Some(destination), None),
                        Err(_) => {
                            set_status(SERVICE_STOPPED, 2, 0);
                            return;
                        }
                    }
                }
                _ => (None, None),
            }
        } else {
            let stdout_destination = if (timestamp_log || rotate_online) && stdout_path.is_some() {
                match (stdout_path.as_deref(), stdout_file.take()) {
                    (Some(path), Some(file)) => {
                        match LogDestination::new(&service_name, "AppStdout", path.to_owned(), file)
                        {
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
                        match LogDestination::new(&service_name, "AppStderr", path.to_owned(), file)
                        {
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
            (stdout_destination, stderr_destination)
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
        let mut creation_flags = priority & PRIORITY_CLASS_MASK;
        if no_console {
            creation_flags |= CREATE_NO_WINDOW;
        } else {
            // Keep the child in the console's default process group so a
            // CTRL_C_EVENT sent with group id 0 reaches its handler.
            if stop_method_skip & STOP_SKIP_CONSOLE == 0 {
                creation_flags |= CREATE_NEW_CONSOLE;
            }
        }
        let explicit_affinity = affinity
            .as_deref()
            .and_then(|value| parse_cpu_affinity(value).ok())
            .is_some_and(|mask| mask != 0);
        if explicit_affinity {
            creation_flags |= CREATE_SUSPENDED;
        }
        command.creation_flags(creation_flags);
        command.current_dir(&directory);
        if app_environment.is_some() {
            command.env_clear();
            for entry in &child_environment {
                if let Some((key, value)) = entry.split_once('=') {
                    command.env(key, value);
                }
            }
        } else if !extra_environment.is_empty() {
            for change in &extra_environment {
                if let Some((key, _)) = change.split_once('=')
                    && let Some(value) = child_environment.iter().rev().find_map(|entry| {
                        entry
                            .split_once('=')
                            .filter(|(name, _)| name.eq_ignore_ascii_case(key))
                            .map(|(_, value)| value)
                    })
                {
                    command.env(key, value);
                }
            }
        }
        let mut process = match command.spawn() {
            Ok(process) => process,
            Err(error) => {
                match nssm_rs::launch_failure_disposition(
                    application_started,
                    STOP_REQUESTED.load(Ordering::SeqCst),
                ) {
                    LaunchFailureDisposition::StopRequested => break,
                    LaunchFailureDisposition::Retry => {
                        retry_failed_restart_launch(
                            &service_name,
                            &app,
                            throttle_ms,
                            restart_delay_ms,
                            &mut throttle,
                        );
                        continue;
                    }
                    LaunchFailureDisposition::StopService => {}
                }
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
        if let Some(destination) = stdout_destination.as_ref()
            && let Some(pipe) = process.stdout.take()
        {
            let destination = destination.clone();
            logger_threads.push(thread::spawn(move || {
                log_stream(pipe, destination, timestamp_stdout)
            }));
        }
        if let Some(destination) = stderr_destination.as_ref()
            && let Some(pipe) = process.stderr.take()
        {
            let destination = destination.clone();
            logger_threads.push(thread::spawn(move || {
                log_stream(pipe, destination, timestamp_stderr)
            }));
        }
        let mut launched = match assign_to_job(process, kill_process_tree) {
            Ok(launched) => launched,
            Err(_) => {
                set_status(SERVICE_STOPPED, 2, 0);
                return;
            }
        };
        let process_handle = launched.child.as_raw_handle() as HANDLE;
        configure_child_affinity(&service_name, process_handle, affinity.as_deref());
        if explicit_affinity && let Err(error) = resume_suspended_child(launched.child.id()) {
            let _ = launched.child.kill();
            report_event(
                EVENTLOG_ERROR_TYPE,
                EVENT_CREATEPROCESS_FAILED,
                &[&service_name, &app, &error],
            );
            set_status_with_service_exit(
                SERVICE_STOPPED,
                windows_sys::Win32::Foundation::ERROR_SERVICE_SPECIFIC_ERROR,
                SERVICE_LAUNCH_FAILURE_CODE,
                0,
            );
            return;
        }
        application_started = true;
        if let Ok(mut child) = CHILD_PROCESS.lock() {
            *child = Some(launched);
        }
        throttle.mark_started();
        HOOK_THROTTLE_COUNT.store(throttle.counter(), Ordering::SeqCst);
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
        let child_started = Instant::now();
        set_status(SERVICE_RUNNING, 0, 0);
        let parameters = read_optional_reg_string(&service_name, "AppParameters")
            .ok()
            .flatten()
            .unwrap_or_default();
        report_event(
            windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
            EVENT_STARTED_SERVICE,
            &[&app, &parameters, &service_name, &directory],
        );
        let mut child_exit_code = 1;
        let mut rotation_check = Instant::now();
        loop {
            if STOP_REQUESTED.load(Ordering::SeqCst) {
                break;
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
                for (index, (destination, path, copy)) in [
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
                ]
                .into_iter()
                .enumerate()
                {
                    if same_output_path && index == 1 {
                        continue;
                    }
                    let rotation = if let Some(destination) = destination {
                        destination.rotate(
                            Duration::from_secs(rotate_seconds as u64),
                            rotate_size,
                            rotate_delay,
                            requested_rotation,
                            copy,
                        )
                    } else if let Some(path) = path {
                        rotate_log_file(
                            path,
                            Duration::from_secs(rotate_seconds as u64),
                            rotate_size,
                            rotate_delay,
                            requested_rotation,
                            copy,
                            false,
                        )
                    } else {
                        continue;
                    };
                    match rotation {
                        Ok(Some(rotated)) => {
                            if let Some(path) = path {
                                report_rotation_event(&service_name, path, &rotated);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let active_path = destination
                                .map(|destination| destination.path.as_str())
                                .or(path);
                            if let Some(path) = active_path {
                                report_rotation_failure(&service_name, path, &error);
                            }
                        }
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
        let mut child_still_active = false;
        if let Ok(mut child) = CHILD_PROCESS.lock()
            && let Some(mut proc) = child.take()
        {
            let stop_requested = STOP_REQUESTED.load(Ordering::SeqCst);
            let mut tree_events_logged = false;
            if stop_requested {
                stop_process_handled = true;
                match stop_child(&service_name, &app, &mut proc, no_console, stop_method_skip) {
                    Ok(StopChildOutcome::Exited {
                        tree_events_logged: logged,
                    }) => tree_events_logged = logged,
                    Ok(StopChildOutcome::StillActive) => {
                        child_still_active = true;
                        child_exit_code = windows_sys::Win32::Foundation::STILL_ACTIVE as u32;
                        if kill_process_tree {
                            report_process_tree_events(
                                &service_name,
                                proc.child.id(),
                                child_exit_code,
                                true,
                            );
                            tree_events_logged = true;
                        }
                    }
                    Err(_) => {}
                }
            }
            if !child_still_active
                && let Ok(status) = proc.child.wait()
                && STOP_REQUESTED.load(Ordering::SeqCst)
            {
                child_exit_code = status.code().unwrap_or(1) as u32;
            }
            if proc.job != 0 && !tree_events_logged {
                report_process_tree_events(&service_name, proc.child.id(), child_exit_code, false);
            }
        }
        if child_still_active {
            // Let redirected output drain until the service host exits; joining
            // here would keep the stopped service alive behind the orphaned child.
            drop(logger_threads);
        } else if logger_threads
            .into_iter()
            .any(|logger| logger.join().map_or(true, |result| result.is_err()))
        {
            child_exit_code = 2;
        }
        HOOK_LAST_APPLICATION_EXIT_CODE.store(child_exit_code, Ordering::SeqCst);
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
        let graceful_default_suicide =
            child_exit_code == 0 && inherited_default && action == ExitAction::Suicide;
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
                let logged_action = if graceful_default_suicide {
                    "Suicide"
                } else {
                    "Exit"
                };
                report_event(
                    windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
                    EVENT_EXIT,
                    &[&service_name, &exit_code, logged_action],
                );
                if graceful_default_suicide {
                    report_event(
                        windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
                        EVENT_GRACEFUL_SUICIDE,
                        &[&service_name, &app, "Suicide", "Suicide", "Suicide", "Exit"],
                    );
                    report_event(
                        windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
                        EVENT_PROCESS_ALREADY_STOPPED,
                        &[&service_name, &app],
                    );
                }
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
        wait_for_restart_delay(
            &service_name,
            delay,
            throttle_ms,
            restart_delay_ms,
            &mut throttle,
        );
    }
    if STOP_REQUESTED.load(Ordering::SeqCst) && !stop_process_handled {
        report_event(
            windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE,
            EVENT_PROCESS_ALREADY_STOPPED,
            &[&service_name, &app],
        );
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
    if let Some(error) = missing_service_error(name) {
        print_open_service_error(&error);
        return Ok(0);
    }
    let handle = match open_service(name, SERVICE_QUERY_STATUS) {
        Ok(handle) => handle,
        Err(error) => {
            cli_eprintln!("{name}: {error}\r");
            return Ok(0);
        }
    };
    let current_state = match state(handle.0) {
        Ok(current_state) => current_state,
        Err(error) => {
            cli_eprintln!("{name}: {error}\r");
            return Ok(0);
        }
    };
    cli_println!("{}\r", state_name(current_state));
    Ok(current_state.min(u8::MAX as u32) as u8)
}

fn is_administrator() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

fn elevation_arguments(command: &Command) -> Option<Vec<String>> {
    match command {
        Command::Install {
            service,
            application,
            args,
        } => {
            let mut arguments = vec!["install".to_owned(), service.clone()];
            if let Some(application) = application {
                arguments.push(application.clone());
                arguments.extend(args.iter().cloned());
            }
            Some(arguments)
        }
        Command::Remove { service, confirm } => {
            let mut arguments = vec!["remove".to_owned(), service.clone()];
            if *confirm {
                arguments.push("confirm".to_owned());
            }
            Some(arguments)
        }
        Command::Dump {
            service,
            new_name: None,
        } => Some(vec!["dump".to_owned(), service.clone()]),
        _ => None,
    }
}

fn elevation_parameters(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| quote_windows_arg(argument))
        .collect::<Vec<_>>()
        .join(" ")
}

fn elevate(arguments: &[String], command: &Command) -> u8 {
    match command {
        Command::Install { .. } => {
            cli_eprintln!("Administrator access is needed to install a service.\r\r");
        }
        Command::Remove { .. } => {
            cli_eprintln!("Administrator access is needed to remove a service.\r\r");
        }
        Command::Dump { .. } => {
            cli_eprintln!("Administrator access is needed to edit a service.\r\r");
        }
        _ => {}
    }
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            cli_eprintln!("nssm-rs: unable to locate executable for elevation: {error}");
            return 100;
        }
    };
    let executable = wide(executable.as_os_str());
    let verb = wide(OsStr::new("runas"));
    let parameters = elevation_parameters(arguments);
    let parameters = wide(OsStr::new(&parameters));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = windows_sys::Win32::UI::Shell::SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = executable.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = SW_SHOW;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        cli_eprintln!("nssm-rs: Windows refused the administrator elevation request");
        100
    } else {
        if info.hProcess.is_null() {
            cli_eprintln!("nssm-rs: Windows did not return the elevated process handle");
            return 100;
        }
        let wait_result = unsafe {
            WaitForSingleObject(
                info.hProcess,
                windows_sys::Win32::System::Threading::INFINITE,
            )
        };
        let mut exit_code = 1;
        let got_exit_code = wait_result == WAIT_OBJECT_0
            && unsafe { GetExitCodeProcess(info.hProcess, &mut exit_code) } != 0;
        unsafe { CloseHandle(info.hProcess) };
        if !got_exit_code {
            cli_eprintln!("nssm-rs: unable to read the elevated command's exit status");
            return 100;
        }
        u8::try_from(exit_code).unwrap_or(100)
    }
}

pub fn execute(command: Command) -> Result<u8, String> {
    if !is_administrator()
        && let Some(arguments) = elevation_arguments(&command)
    {
        return Ok(elevate(&arguments, &command));
    }
    let service = match &command {
        Command::Start(service)
        | Command::Stop(service)
        | Command::Restart(service)
        | Command::Pause(service)
        | Command::Continue(service)
        | Command::Status(service)
        | Command::Rotate(service)
        | Command::Get { service, .. }
        | Command::Set { service, .. }
        | Command::Reset { service, .. }
        | Command::Unset { service, .. }
        | Command::Remove { service, .. }
        | Command::Dump { service, .. } => Some(service.as_str()),
        Command::Processes(_) => None,
        Command::Install { .. }
        | Command::List { .. }
        | Command::StatusCode(_)
        | Command::Help
        | Command::Version => None,
    };
    let invalid_parameter = match &command {
        Command::Get { parameter, .. }
        | Command::Set { parameter, .. }
        | Command::Reset { parameter, .. }
        | Command::Unset { parameter, .. }
            if !known_setting(parameter) =>
        {
            Some(parameter.as_str())
        }
        _ => None,
    };
    if let Some(parameter) = invalid_parameter {
        print_invalid_parameter(parameter);
        return Ok(1);
    }
    if let Some(service) = service
        && let Some(error) = missing_service_error(service)
    {
        print_open_service_error(&error);
        return Ok(3);
    }
    if let Command::Get {
        parameter, extra, ..
    } = &command
        && extra.is_empty()
        && (parameter.eq_ignore_ascii_case("AppExit")
            || parameter.eq_ignore_ascii_case("AppEvents"))
    {
        cli_eprintln!("Parameter \"{parameter}\" requires a subparameter!\r\r");
        return Ok(1);
    }
    let managed_parameter = match &command {
        Command::Get {
            service, parameter, ..
        }
        | Command::Set {
            service, parameter, ..
        }
        | Command::Reset {
            service, parameter, ..
        }
        | Command::Unset {
            service, parameter, ..
        } if managed_setting(parameter).is_some() => Some((service.as_str(), parameter.as_str())),
        _ => None,
    };
    if let Some((service, parameter)) = managed_parameter
        && !is_nssm_service(service)
    {
        cli_eprintln!("Parameter \"{parameter}\" is only valid for services managed by NSSM!\r\r");
        return Ok(1);
    }
    match command {
        Command::StatusCode(name) => status_code(&name),
        Command::Set {
            service,
            parameter,
            values,
        } if parameter.eq_ignore_ascii_case("AppAffinity") => {
            if values.is_empty() {
                return Err("set AppAffinity requires a value".into());
            }
            if let Err(error) = open_service(&service, SERVICE_QUERY_CONFIG | SERVICE_CHANGE_CONFIG)
            {
                cli_eprintln!("nssm-rs: {error}\r");
                return Ok(3);
            }
            if !is_nssm_service(&service) {
                cli_eprintln!("nssm-rs: AppAffinity is only valid for nssm-rs services\r");
                return Ok(1);
            }
            match set_setting(&service, &parameter, &values) {
                Ok(()) => Ok(0),
                Err(error) => {
                    cli_eprintln!("nssm-rs: {error}\r");
                    Ok(6)
                }
            }
        }
        Command::Dump { service, new_name } => match dump_service(&service, new_name.as_deref()) {
            Ok(()) => Ok(0),
            Err((code, error)) => {
                if error.starts_with("Can't open service!\r") {
                    write_cli_output(&error, true);
                } else {
                    cli_eprintln!("nssm-rs: {error}");
                }
                Ok(code)
            }
        },
        command => match command {
            Command::Processes(services) => match processes(&services) {
                Ok(()) => Ok(0),
                Err(error) => {
                    if !error.is_empty() {
                        cli_eprintln!("{error}\r");
                    }
                    Ok(1)
                }
            },
            Command::Get {
                service,
                parameter,
                extra,
            } if known_setting(&parameter) => match get_setting(&service, &parameter, &extra) {
                Ok(()) => Ok(0),
                Err(error) => {
                    cli_eprintln!("nssm-rs: {error}\r");
                    Ok(5)
                }
            },
            Command::Set {
                service,
                parameter,
                values,
            } if known_setting(&parameter) => match set_setting(&service, &parameter, &values) {
                Ok(()) => Ok(0),
                Err(error) => {
                    cli_eprintln!("nssm-rs: {error}\r");
                    Ok(6)
                }
            },
            Command::Reset {
                service,
                parameter,
                extra,
            }
            | Command::Unset {
                service,
                parameter,
                extra,
            } if known_setting(&parameter) => match reset_setting(&service, &parameter, &extra) {
                Ok(()) => Ok(0),
                Err(error) => {
                    cli_eprintln!("nssm-rs: {error}\r");
                    Ok(6)
                }
            },
            command => execute_service_command(command).map(|()| 0),
        },
    }
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
            cli_println!("{}\r", state_name(s));
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
        Command::Dump { .. } => unreachable!("dump commands are handled with NSSM exit codes"),
        Command::List { all } => list_services(all),
        Command::Processes(names) => processes(&names),
        _ => Err(format!("command is not implemented yet: {command:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Command, EVENT_GET_PROCESS_AFFINITY_MASK_FAILED, EVENT_OPENSCMANAGER_FAILED,
        EVENT_SERVICE_CONTROL_HANDLED, EVENT_SERVICE_CONTROL_NOT_HANDLED,
        EVENT_SERVICE_CONTROL_UNKNOWN, EVENT_SET_PROCESS_AFFINITY_MASK_FAILED,
        FAIL_NEXT_PROCESS_ENUMERATION, FAIL_NEXT_PROCESS_SNAPSHOT, FAIL_NEXT_PROCESS_TIMES,
        FAIL_NEXT_STDIO_DUPLICATE, FAIL_OPEN_PROCESS_PID, HOOK_LAST_APPLICATION_EXIT_CODE,
        HookAbiContext, Ordering, PBT_APMPOWERSTATUSCHANGE, PBT_APMRESUMEAUTOMATIC,
        PowerHookAction, PowerHookInvocation, SERVICE_CHILD_ENVIRONMENT, SERVICE_CONTROL_PAUSE,
        SERVICE_CONTROL_POWEREVENT, StdioFileError, configure_child_affinity,
        dump_open_service_error, duplicate_stdio_file, elevation_arguments, elevation_parameters,
        ensure_event_source, finish_async_hooks, hook_abi_environment, hook_deadline_ms,
        hook_exit_code, hook_trigger, install, is_administrator, open_manager, parse_u32,
        power_hook_action, power_hook_invocation, priority_name, priority_value,
        process_belongs_to_service_lifetime, process_open_error_output, process_tree_from_root,
        quote_dump_arg, remove, report_stdio_setup_failure, service_control_event, service_handler,
        service_main, service_type_name, set_setting, valid_hook, valid_priority_class, wide,
    };

    static WINDOWS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn powershell_path() -> String {
        let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
    }

    fn latest_event_record(powershell: &str, event_id: u32) -> u64 {
        let script = format!(
            "$event = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id={event_id}}} -MaxEvents 1 -ErrorAction SilentlyContinue | Select-Object -First 1; if ($event) {{ [Console]::Write($event.RecordId) }} else {{ [Console]::Write('0') }}"
        );
        let output = std::process::Command::new(powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .expect("query the latest event record");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .expect("PowerShell returns the prior event record number")
    }

    #[test]
    fn administrator_scm_open_failure_is_written_to_the_application_log() {
        let event_id = EVENT_OPENSCMANAGER_FAILED & 0xffff;
        let event_record = if is_administrator() {
            ensure_event_source().expect("the test runner can register the NSSM event source");
            Some(latest_event_record(&powershell_path(), event_id))
        } else {
            None
        };

        assert!(open_manager(u32::MAX).is_err());

        if let Some(before) = event_record {
            wait_for_event(&powershell_path(), event_id, before, "");
        }
    }

    #[test]
    fn affinity_api_failures_report_reference_events_1069_and_1070() {
        let _test_lock = WINDOWS_TEST_LOCK
            .lock()
            .expect("Windows event tests are serialized");
        ensure_event_source().expect("test runner can register the NSSM event source");
        let powershell = powershell_path();
        let get_event = EVENT_GET_PROCESS_AFFINITY_MASK_FAILED & 0xffff;
        let set_event = EVENT_SET_PROCESS_AFFINITY_MASK_FAILED & 0xffff;
        let get_before = latest_event_record(&powershell, get_event);
        let set_before = latest_event_record(&powershell, set_event);
        let service = "NssmRsAffinityApiFailureTest";

        configure_child_affinity(service, std::ptr::null_mut(), Some("0"));

        wait_for_event(&powershell, get_event, get_before, service);
        wait_for_event(&powershell, set_event, set_before, service);
    }

    fn wait_for_event(powershell: &str, event_id: u32, after: u64, text: &str) {
        let script = format!(
            "$event = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id={event_id}}} -MaxEvents 30 -ErrorAction SilentlyContinue | Where-Object {{ $_.RecordId -gt {after} -and $_.Message.Contains('{text}') }} | Select-Object -First 1; if ($event) {{ exit 0 }}; exit 1"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if std::process::Command::new(powershell)
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .status()
                .is_ok_and(|status| status.success())
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        panic!("Application log contains event {event_id} mentioning {text}");
    }

    struct TestDirectory(std::path::PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time is after the Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "nssm-rs-rotation-test-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("temporary test directory is created");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct TestService {
        name: String,
        last_control: u32,
        last_exit_code: u32,
        child_environment: Vec<String>,
    }

    impl TestService {
        fn new(name: String) -> Self {
            Self {
                name,
                last_control: super::HOOK_LAST_CONTROL.load(std::sync::atomic::Ordering::SeqCst),
                last_exit_code: HOOK_LAST_APPLICATION_EXIT_CODE
                    .load(std::sync::atomic::Ordering::SeqCst),
                child_environment: SERVICE_CHILD_ENVIRONMENT
                    .lock()
                    .map(|environment| environment.clone())
                    .unwrap_or_default(),
            }
        }
    }

    impl Drop for TestService {
        fn drop(&mut self) {
            finish_async_hooks();
            if let Ok(mut environment) = SERVICE_CHILD_ENVIRONMENT.lock() {
                *environment = std::mem::take(&mut self.child_environment);
            }
            HOOK_LAST_APPLICATION_EXIT_CODE
                .store(self.last_exit_code, std::sync::atomic::Ordering::SeqCst);
            super::HOOK_LAST_CONTROL.store(self.last_control, std::sync::atomic::Ordering::SeqCst);
            let _ = remove(&self.name, true);
        }
    }

    #[test]
    fn rotation_collision_returns_move_operation_and_preserves_active_log() {
        let directory = TestDirectory::new();
        let active = directory.0.join("active.log");
        std::fs::write(&active, "active log data").expect("active log is written");
        let modified = std::fs::metadata(&active)
            .and_then(|metadata| metadata.modified())
            .expect("active log modification time is readable");
        let rotated = super::rotated_log_path(&active, modified)
            .expect("rotation target is generated from the active log timestamp");
        std::fs::create_dir(&rotated).expect("rotation target collision is created");
        let active = active.to_string_lossy().into_owned();

        let error = super::rotate_log_file(
            &active,
            std::time::Duration::ZERO,
            0,
            std::time::Duration::ZERO,
            false,
            false,
            false,
        )
        .expect_err("renaming onto an existing directory fails");

        assert_eq!(error.operation, "MoveFile()");
        assert_eq!(error.target, rotated.to_string_lossy());
        assert!(!error.message.is_empty());
        assert!(rotated.is_dir(), "collision target is left intact");
        assert_eq!(
            std::fs::read_to_string(&active).expect("active log remains available"),
            "active log data"
        );
    }

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
    fn elevation_rebuilds_install_remove_and_dump_arguments_safely() {
        let install = Command::Install {
            service: "worker service".into(),
            application: Some(r"C:\Program Files\worker.exe".into()),
            args: vec!["--label".into(), "two words".into()],
        };
        let install_arguments = elevation_arguments(&install).expect("install requests elevation");
        assert_eq!(
            elevation_parameters(&install_arguments),
            r#"install "worker service" "C:\Program Files\worker.exe" --label "two words""#
        );

        let remove = Command::Remove {
            service: "worker service".into(),
            confirm: true,
        };
        let remove_arguments = elevation_arguments(&remove).expect("remove requests elevation");
        assert_eq!(remove_arguments, ["remove", "worker service", "confirm"]);

        let dump = Command::Dump {
            service: "Task Scheduler".into(),
            new_name: None,
        };
        let dump_arguments = elevation_arguments(&dump).expect("single-service dump elevates");
        assert_eq!(dump_arguments, ["dump", "Task Scheduler"]);
        assert!(
            elevation_arguments(&Command::Dump {
                service: "Task Scheduler".into(),
                new_name: Some("copy".into()),
            })
            .is_none()
        );

        assert!(elevation_arguments(&Command::List { all: false }).is_none());
    }

    #[test]
    fn dump_quoting_escapes_cmd_special_characters_and_preserves_windows_arguments() {
        assert_eq!(
            quote_dump_arg("/c ping -n 30 127.0.0.1 > nul"),
            "^\"/c ping -n 30 127.0.0.1 ^> nul^\""
        );
        assert_eq!(quote_dump_arg("two words"), "\"two words\"");
        assert_eq!(
            quote_dump_arg("C:\\Program Files\\app\\"),
            "\"C:\\Program Files\\app\\\\\""
        );
        assert_eq!(quote_dump_arg("plain"), "plain");
    }

    #[test]
    fn process_tree_excludes_pids_reused_outside_service_lifetime() {
        assert!(process_belongs_to_service_lifetime(100, 200, 100));
        assert!(process_belongs_to_service_lifetime(100, 200, 150));
        assert!(process_belongs_to_service_lifetime(100, 200, 200));
        assert!(!process_belongs_to_service_lifetime(100, 200, 99));
        assert!(!process_belongs_to_service_lifetime(100, 200, 201));
    }

    #[test]
    fn process_open_error_uses_windows_message_and_nssm_line_endings() {
        let error = std::io::Error::from_raw_os_error(5);
        let output = process_open_error_output("Spooler", 5280, &error);
        assert!(output.starts_with("Spooler: 5280: "));
        assert!(!output.contains("(os error 5)"));
        assert!(output.ends_with("\r\r\n\r\n"));
    }

    #[test]
    fn dump_access_denied_matches_nssm_privilege_diagnostic() {
        let error = dump_open_service_error("Access is denied. (os error 5)");
        assert!(error.starts_with("Can't open service!\r\r\nOpenService(): "));
        assert!(error.ends_with("\r\r\n\r\r\n"));
        assert!(!error.contains("os error 5"));
        assert_eq!(
            dump_open_service_error("unexpected failure"),
            "unexpected failure"
        );
    }

    #[test]
    fn service_main_reports_event_1005_when_handler_registration_fails() {
        let _test_lock = WINDOWS_TEST_LOCK
            .lock()
            .expect("Windows event tests are serialized");
        ensure_event_source().expect("test runner can register the NSSM event source");
        let service_name = format!(
            "NssmRsHandlerFailure{}{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time is after the Unix epoch")
                .as_millis()
        );
        let name = wide(std::ffi::OsStr::new(&service_name));
        let mut name_pointer = name.as_ptr() as *mut u16;
        let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let latest_event = "$event = Get-WinEvent -FilterHashtable @{LogName='Application'; ProviderName='nssm'; Id=1005} -MaxEvents 1 -ErrorAction SilentlyContinue | Select-Object -First 1; if ($event) { [Console]::Write($event.RecordId) } else { [Console]::Write('0') }";
        let previous_record = std::process::Command::new(&powershell)
            .args(["-NoProfile", "-NonInteractive", "-Command", latest_event])
            .output()
            .expect("query the latest event record");
        assert!(previous_record.status.success());
        let previous_record = String::from_utf8_lossy(&previous_record.stdout)
            .trim()
            .parse::<u64>()
            .expect("PowerShell returns the prior event record number");

        unsafe { service_main(1, &mut name_pointer) };

        let script = format!(
            "$event = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1005}} -MaxEvents 30 -ErrorAction SilentlyContinue | Where-Object {{ $_.RecordId -gt {previous_record} -and $_.Message.Contains('RegisterServiceCtrlHandlerEx() failed') }} | Select-Object -First 1; if ($event) {{ exit 0 }}; exit 1"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if std::process::Command::new(&powershell)
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .status()
                .is_ok_and(|status| status.success())
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        panic!("Application log contains event 1005 for handler registration failure");
    }

    #[test]
    fn process_snapshot_failure_reports_event_1024_and_returns_the_windows_error() {
        let _test_lock = WINDOWS_TEST_LOCK
            .lock()
            .expect("Windows event tests are serialized");
        ensure_event_source().expect("test runner can register the NSSM event source");
        let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let before = std::process::Command::new(&powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$event = Get-WinEvent -FilterHashtable @{LogName='Application'; ProviderName='nssm'; Id=1024} -MaxEvents 1 -ErrorAction SilentlyContinue | Select-Object -First 1; if ($event) { [Console]::Write($event.RecordId) } else { [Console]::Write('0') }",
            ])
            .output()
            .expect("query the latest event record");
        assert!(before.status.success());
        let before = String::from_utf8_lossy(&before.stdout)
            .trim()
            .parse::<u64>()
            .expect("PowerShell returns the prior event record number");

        FAIL_NEXT_PROCESS_SNAPSHOT.store(true, Ordering::SeqCst);
        let error = process_tree_from_root("NssmRsSnapshotFailureTest", std::process::id())
            .expect_err("injected snapshot failure is returned to the caller");
        assert!(!error.is_empty());

        let query = format!(
            "$event = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1024}} -MaxEvents 30 -ErrorAction SilentlyContinue | Where-Object {{ $_.RecordId -gt {before} -and $_.Message.Contains('NssmRsSnapshotFailureTest') }} | Select-Object -First 1; if ($event) {{ exit 0 }}; exit 1"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if std::process::Command::new(&powershell)
                .args(["-NoProfile", "-NonInteractive", "-Command", &query])
                .status()
                .is_ok_and(|status| status.success())
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        panic!("Application log contains event 1024 for the injected snapshot failure");
    }

    #[test]
    fn process_enumeration_failure_reports_event_1025() {
        let _test_lock = WINDOWS_TEST_LOCK
            .lock()
            .expect("Windows event tests are serialized");
        ensure_event_source().expect("test runner can register the NSSM event source");
        let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let before = std::process::Command::new(&powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$event = Get-WinEvent -FilterHashtable @{LogName='Application'; ProviderName='nssm'; Id=1025} -MaxEvents 1 -ErrorAction SilentlyContinue | Select-Object -First 1; if ($event) { [Console]::Write($event.RecordId) } else { [Console]::Write('0') }",
            ])
            .output()
            .expect("query the latest event record");
        assert!(before.status.success());
        let before = String::from_utf8_lossy(&before.stdout)
            .trim()
            .parse::<u64>()
            .expect("PowerShell returns the prior event record number");

        FAIL_NEXT_PROCESS_ENUMERATION.store(true, Ordering::SeqCst);
        process_tree_from_root("NssmRsEnumerationFailureTest", std::process::id())
            .expect("process enumeration failure is reported and traversal returns");

        let query = format!(
            "$event = Get-WinEvent -FilterHashtable @{{LogName='Application'; ProviderName='nssm'; Id=1025}} -MaxEvents 30 -ErrorAction SilentlyContinue | Where-Object {{ $_.RecordId -gt {before} -and $_.Message.Contains('NssmRsEnumerationFailureTest') }} | Select-Object -First 1; if ($event) {{ exit 0 }}; exit 1"
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if std::process::Command::new(&powershell)
                .args(["-NoProfile", "-NonInteractive", "-Command", &query])
                .status()
                .is_ok_and(|status| status.success())
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        panic!("Application log contains event 1025 for injected enumeration failure");
    }

    #[test]
    fn process_image_open_failure_reports_event_1026_for_a_descendant() {
        let _test_lock = WINDOWS_TEST_LOCK
            .lock()
            .expect("Windows event tests are serialized");
        ensure_event_source().expect("test runner can register the NSSM event source");
        let powershell = powershell_path();
        let before = latest_event_record(&powershell, 1026);
        let mut child = std::process::Command::new(&powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ])
            .spawn()
            .expect("start a disposable descendant process");
        FAIL_OPEN_PROCESS_PID.store(child.id(), Ordering::SeqCst);

        let result = process_tree_from_root("NssmRsOpenProcessFailureTest", std::process::id());
        let _ = child.kill();
        let _ = child.wait();
        result.expect("descendant OpenProcess failure is reported and traversal returns");
        wait_for_event(&powershell, 1026, before, "NssmRsOpenProcessFailureTest");
    }

    #[test]
    fn process_times_failure_reports_event_1044() {
        let _test_lock = WINDOWS_TEST_LOCK
            .lock()
            .expect("Windows event tests are serialized");
        ensure_event_source().expect("test runner can register the NSSM event source");
        let powershell = powershell_path();
        let before = latest_event_record(&powershell, 1044);
        FAIL_NEXT_PROCESS_TIMES.store(true, Ordering::SeqCst);

        process_tree_from_root("NssmRsProcessTimesFailureTest", std::process::id())
            .expect("GetProcessTimes failure is reported and traversal returns");
        wait_for_event(&powershell, 1044, before, "");
    }

    #[test]
    fn duplicate_stdio_handle_failure_reports_event_1050() {
        let _test_lock = WINDOWS_TEST_LOCK
            .lock()
            .expect("Windows event tests are serialized");
        ensure_event_source().expect("test runner can register the NSSM event source");
        let powershell = powershell_path();
        let before = latest_event_record(&powershell, 1050);
        let directory = TestDirectory::new();
        let file = std::fs::File::create(directory.0.join("shared-output.log"))
            .expect("create a disposable output handle");
        FAIL_NEXT_STDIO_DUPLICATE.store(true, Ordering::SeqCst);

        let error = duplicate_stdio_file(&file).expect_err("injected handle duplication fails");
        assert_eq!(error.raw_os_error(), Some(5));
        let error = StdioFileError::Duplicate {
            source: "stdout",
            destination: "stderr",
            error,
        };
        report_stdio_setup_failure("NssmRsDuplicateHandleFailureTest", "fixture.exe", &error);

        wait_for_event(&powershell, 1050, before, "stdout");
    }

    #[test]
    fn power_callback_launches_resume_hook_with_the_nssm_environment() {
        let _test_lock = WINDOWS_TEST_LOCK
            .lock()
            .expect("Windows event tests are serialized");
        let service = format!(
            "NssmRsPowerCallback{}{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time is after the Unix epoch")
                .as_millis()
        );
        let _cleanup = TestService::new(service.clone());
        let application = std::env::current_exe()
            .expect("test executable path is available")
            .to_string_lossy()
            .into_owned();
        install(&service, Some(&application), &[])
            .expect("temporary service configuration is installed");

        let directory = TestDirectory::new();
        let output = directory.0.join("power-hook-environment.txt");
        let output = output.to_string_lossy().replace('\'', "''");
        let windows = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        let powershell = format!("{windows}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let script = format!(
            "[IO.File]::WriteAllLines('{output}', @($env:NSSM_EVENT, $env:NSSM_ACTION, $env:NSSM_TRIGGER, $env:NSSM_LAST_CONTROL, $env:NSSM_EXITCODE))"
        );
        let command = format!("\"{powershell}\" -NoProfile -NonInteractive -Command \"{script}\"");
        set_setting(&service, "AppEvents", &["Power/Resume".into(), command])
            .expect("Power/Resume hook is configured");
        *SERVICE_CHILD_ENVIRONMENT
            .lock()
            .expect("service environment lock is available") = std::env::vars_os()
            .map(|(key, value)| format!("{}={}", key.to_string_lossy(), value.to_string_lossy()))
            .collect();
        HOOK_LAST_APPLICATION_EXIT_CODE.store(23, std::sync::atomic::Ordering::SeqCst);

        let service_name = wide(std::ffi::OsStr::new(&service));
        let result = unsafe {
            service_handler(
                SERVICE_CONTROL_POWEREVENT,
                PBT_APMRESUMEAUTOMATIC,
                std::ptr::null_mut(),
                service_name.as_ptr().cast_mut().cast(),
            )
        };
        assert_eq!(result, windows_sys::Win32::Foundation::NO_ERROR);
        finish_async_hooks();

        let output = std::fs::read_to_string(output)
            .expect("Power callback starts a hook that writes its ABI environment");
        assert_eq!(
            output.lines().collect::<Vec<_>>(),
            ["Power", "Resume", "POWEREVENT", "POWEREVENT", "23"]
        );
    }

    #[test]
    fn power_events_map_only_supported_windows_broadcasts_to_hooks() {
        assert_eq!(
            power_hook_action(PBT_APMPOWERSTATUSCHANGE),
            Some(PowerHookAction::Change)
        );
        assert_eq!(
            power_hook_action(PBT_APMRESUMEAUTOMATIC),
            Some(PowerHookAction::Resume)
        );
        assert_eq!(power_hook_action(0), None);
        assert_eq!(power_hook_action(4), None);
        assert_eq!(power_hook_action(7), None);
        assert_eq!(power_hook_action(0xffff), None);
    }

    #[test]
    fn power_hook_dispatch_uses_the_scm_control_and_nssm_async_policy() {
        assert_eq!(
            power_hook_invocation(PBT_APMPOWERSTATUSCHANGE),
            Some(PowerHookInvocation {
                action: PowerHookAction::Change,
                control: windows_sys::Win32::System::Services::SERVICE_CONTROL_POWEREVENT,
                synchronous: false,
                cancel_on_stop: false,
            })
        );
        assert_eq!(
            power_hook_invocation(PBT_APMRESUMEAUTOMATIC),
            Some(PowerHookInvocation {
                action: PowerHookAction::Resume,
                control: windows_sys::Win32::System::Services::SERVICE_CONTROL_POWEREVENT,
                synchronous: false,
                cancel_on_stop: false,
            })
        );
        assert_eq!(power_hook_invocation(7), None);
    }

    #[test]
    fn power_hook_actions_use_nssm_hook_names() {
        assert_eq!(PowerHookAction::Change.hook_action(), "Change");
        assert_eq!(PowerHookAction::Resume.hook_action(), "Resume");
    }

    #[test]
    fn power_hook_abi_uses_control_trigger_and_general_hook_deadline() {
        for event_type in [PBT_APMPOWERSTATUSCHANGE, PBT_APMRESUMEAUTOMATIC] {
            let invocation = power_hook_invocation(event_type).expect("supported power event");
            assert_eq!(hook_trigger("Power", invocation.control), "POWEREVENT");
            assert_eq!(
                super::control_name(invocation.control),
                "POWEREVENT",
                "NSSM_LAST_CONTROL is the SCM power-event name"
            );
            assert_eq!(
                hook_deadline_ms("Power", invocation.action.hook_action()),
                60_000,
                "Power hooks use the general hook deadline"
            );
        }
    }

    #[test]
    fn power_hook_exit_code_is_empty_while_application_runs_and_retained_after_exit() {
        assert_eq!(hook_exit_code(Some(900), 17), None);
        assert_eq!(hook_exit_code(None, 17), Some(17));
    }

    #[test]
    fn power_hook_environment_matches_the_complete_nssm_abi() {
        let actual = hook_abi_environment(HookAbiContext {
            event: "Power",
            action: "Resume",
            trigger: "POWEREVENT",
            configuration: "64-bit debug",
            build_date: "2026-10-15",
            service: "worker",
            display_name: "Worker service",
            executable: r"C:\Program Files\nssm-rs.exe",
            version: "0.1.0",
            pid: 42,
            last_control: "POWEREVENT",
            start_requested_count: 3,
            start_count: 2,
            throttle_count: 1,
            exit_count: 4,
            runtime: "123",
            application_runtime: "45",
            application_pid: Some(99),
            exit_code: None,
            command_line: r#""C:\Worker\worker.exe" --mode service"#,
            deadline: 60_000,
        })
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
        let expected = [
            ("NSSM_HOOK_VERSION", "1"),
            ("NSSM_EVENT", "Power"),
            ("NSSM_ACTION", "Resume"),
            ("NSSM_TRIGGER", "POWEREVENT"),
            ("NSSM_CONFIGURATION", "64-bit debug"),
            ("NSSM_BUILD_DATE", "2026-10-15"),
            ("NSSM_SERVICE_NAME", "worker"),
            ("NSSM_SERVICE_DISPLAYNAME", "Worker service"),
            ("NSSM_EXE", r"C:\Program Files\nssm-rs.exe"),
            ("NSSM_VERSION", "0.1.0"),
            ("NSSM_PID", "42"),
            ("NSSM_LAST_CONTROL", "POWEREVENT"),
            ("NSSM_START_REQUESTED_COUNT", "3"),
            ("NSSM_START_COUNT", "2"),
            ("NSSM_THROTTLE_COUNT", "1"),
            ("NSSM_EXIT_COUNT", "4"),
            ("NSSM_RUNTIME", "123"),
            ("NSSM_APPLICATION_RUNTIME", "45"),
            ("NSSM_APPLICATION_PID", "99"),
            ("NSSM_EXITCODE", ""),
            (
                "NSSM_COMMAND_LINE",
                r#""C:\Worker\worker.exe" --mode service"#,
            ),
            ("NSSM_DEADLINE", "60000"),
        ]
        .into_iter()
        .map(|(key, value)| (key, value.to_owned()))
        .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(actual, expected);
    }

    #[test]
    fn service_type_names_match_nssm_for_all_supported_types() {
        use windows_sys::Win32::System::Services as services;

        assert_eq!(
            service_type_name(services::SERVICE_KERNEL_DRIVER),
            "SERVICE_KERNEL_DRIVER"
        );
        assert_eq!(
            service_type_name(services::SERVICE_FILE_SYSTEM_DRIVER),
            "SERVICE_FILE_SYSTEM_DRIVER"
        );
        assert_eq!(
            service_type_name(services::SERVICE_WIN32_OWN_PROCESS),
            "SERVICE_WIN32_OWN_PROCESS"
        );
        assert_eq!(
            service_type_name(services::SERVICE_WIN32_SHARE_PROCESS),
            "SERVICE_WIN32_SHARE_PROCESS"
        );
        assert_eq!(
            service_type_name(services::SERVICE_WIN32_OWN_PROCESS | 0x100),
            "SERVICE_INTERACTIVE_PROCESS"
        );
        assert_eq!(
            service_type_name(services::SERVICE_WIN32_SHARE_PROCESS | 0x100),
            "SERVICE_WIN32_SHARE_PROCESS|SERVICE_INTERACTIVE_PROCESS"
        );
        assert_eq!(service_type_name(0xffff), "?");
    }

    #[test]
    fn service_control_events_distinguish_handled_unsupported_and_unknown_codes() {
        assert_eq!(
            service_control_event(0, true),
            (EVENT_SERVICE_CONTROL_HANDLED, "START".into())
        );
        assert_eq!(
            service_control_event(SERVICE_CONTROL_PAUSE, false),
            (EVENT_SERVICE_CONTROL_NOT_HANDLED, "PAUSE".into())
        );
        assert_eq!(
            service_control_event(200, false),
            (EVENT_SERVICE_CONTROL_UNKNOWN, "0x000000c8".into())
        );
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
    fn priority_class_mask_rejects_unrelated_flags_and_preserves_reference_values() {
        for value in [0x20, 0x40, 0x80, 0x100, 0x4000, 0x8000, 0xc1e0] {
            assert!(valid_priority_class(value), "{value:#x}");
        }
        assert!(!valid_priority_class(0x1_0000));
        assert!(!valid_priority_class(0x20 | 0x1_0000));
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
