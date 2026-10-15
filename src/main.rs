use nssm_rs::{Command, VERSION, parse_args};
use std::process::ExitCode;

const USAGE: &str = "nssm-rs: Windows service wrapper\n\nUsage: nssm-rs <command> [arguments]\n\nCommands: install, remove, start, stop, restart, pause, continue, status, statuscode, get, set, reset, unset, rotate, list, dump, processes, help, version\n";

fn run(args: &[String]) -> Result<u8, String> {
    match parse_args(args).map_err(|e| e.to_string())? {
        Command::Help => {
            #[cfg(windows)]
            print!("{}", USAGE.replace('\n', "\r\n"));
            #[cfg(not(windows))]
            print!("{USAGE}");
            Ok(0)
        }
        Command::Version => {
            #[cfg(windows)]
            println!("nssm-rs {VERSION}\r");
            #[cfg(not(windows))]
            println!("nssm-rs {VERSION}");
            Ok(0)
        }
        command => {
            #[cfg(not(windows))]
            {
                let _ = command;
                Err("nssm-rs manages Windows services and must run on Windows".into())
            }
            #[cfg(windows)]
            {
                windows_backend::execute(command)
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "__service") {
        #[cfg(windows)]
        {
            return match args.get(1).map(|s| crate::windows::run_service(s)) {
                Some(Ok(())) => ExitCode::SUCCESS,
                Some(Err(error)) => {
                    eprintln!("nssm-rs service host: {error}\r");
                    ExitCode::FAILURE
                }
                None => ExitCode::FAILURE,
            };
        }
        #[cfg(not(windows))]
        {
            return ExitCode::FAILURE;
        }
    }
    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            #[cfg(windows)]
            eprintln!("nssm-rs: {e}\r");
            #[cfg(not(windows))]
            eprintln!("nssm-rs: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
mod windows_backend {
    use nssm_rs::Command;
    pub fn execute(command: Command) -> Result<u8, String> {
        crate::windows::execute(command)
    }
}

#[cfg(windows)]
mod windows;
