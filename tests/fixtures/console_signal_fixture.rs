#[cfg(windows)]
fn main() {
    use std::{
        sync::atomic::{AtomicU32, Ordering},
        thread,
        time::Duration,
    };
    use windows_sys::{
        Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler},
        core::BOOL,
    };

    static SIGNAL: AtomicU32 = AtomicU32::new(0);
    unsafe extern "system" fn handler(event: u32) -> BOOL {
        SIGNAL.store(event + 1, Ordering::SeqCst);
        1
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().is_some_and(|mode| mode == "capture-args") {
        let output = arguments.get(1).expect("argument capture path configured");
        std::fs::write(
            output,
            arguments
                .iter()
                .skip(2)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .expect("write captured arguments");
        return;
    }

    let marker = std::env::var_os("NSSM_SIGNAL_MARKER").expect("marker configured");
    let started = std::env::var_os("NSSM_STARTED_MARKER").expect("startup marker configured");
    match arguments.first().map(String::as_str) {
        Some("thread") => {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetMessageW, MSG, PM_NOREMOVE, PeekMessageW,
            };

            let mut message = std::mem::MaybeUninit::<MSG>::zeroed();
            unsafe {
                PeekMessageW(
                    message.as_mut_ptr(),
                    std::ptr::null_mut(),
                    0,
                    0,
                    PM_NOREMOVE,
                );
            }
            std::fs::write(started, b"started").expect("write startup marker");
            let mut message = unsafe { message.assume_init() };
            while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {}
            std::fs::write(marker, b"thread-quit").expect("write thread quit marker");
            return;
        }
        Some("window") => run_window_mode(marker, started),
        _ => {}
    }
    assert_ne!(unsafe { SetConsoleCtrlHandler(Some(handler), 1) }, 0);
    std::fs::write(started, b"started").expect("write startup marker");
    loop {
        let signal = SIGNAL.load(Ordering::SeqCst);
        if signal != 0 {
            let name = match signal - 1 {
                CTRL_C_EVENT => "ctrl-c",
                CTRL_BREAK_EVENT => "ctrl-break",
                _ => "other",
            };
            std::fs::write(marker, name).expect("write signal marker");
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
fn run_window_mode(marker: std::ffi::OsString, started: std::ffi::OsString) -> ! {
    use std::sync::OnceLock;
    use windows_sys::Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PostMessageW,
            PostQuitMessage, RegisterClassW, TranslateMessage, WM_APP, WM_CLOSE, WNDCLASSW,
            WS_VISIBLE,
        },
    };

    static MARKER: OnceLock<std::ffi::OsString> = OnceLock::new();
    static STARTED: OnceLock<std::ffi::OsString> = OnceLock::new();
    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_APP + 1 {
            if let Some(started) = STARTED.get() {
                let _ = std::fs::write(started, b"started");
            }
            0
        } else if message == WM_CLOSE {
            if let Some(marker) = MARKER.get() {
                let _ = std::fs::write(marker, b"window-close");
            }
            unsafe { PostQuitMessage(0) };
            0
        } else {
            unsafe { DefWindowProcW(window, message, wparam, lparam) }
        }
    }

    let _ = MARKER.set(marker);
    let _ = STARTED.set(started);
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) } as HINSTANCE;
    let class_name = wide("NssmRShutdownFixtureWindow");
    let window_class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    assert_ne!(
        unsafe { RegisterClassW(&window_class) },
        0,
        "window class registers"
    );
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_VISIBLE,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    assert!(!window.is_null(), "hidden top-level window is created");
    unsafe { PostMessageW(window, WM_APP + 1, 0, 0) };
    let mut message = std::mem::MaybeUninit::<MSG>::zeroed();
    while unsafe { GetMessageW(message.as_mut_ptr(), std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(message.as_ptr());
            DispatchMessageW(message.as_ptr());
        }
    }
    std::process::exit(0)
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(not(windows))]
fn main() {}
