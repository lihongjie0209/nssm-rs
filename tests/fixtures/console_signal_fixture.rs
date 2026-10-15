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
    let marker = std::env::var_os("NSSM_SIGNAL_MARKER").expect("marker configured");
    let started = std::env::var_os("NSSM_STARTED_MARKER").expect("startup marker configured");
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

#[cfg(not(windows))]
fn main() {}
