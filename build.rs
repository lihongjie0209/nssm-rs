#[path = "src/build_date.rs"]
mod build_date;

use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn find_tool(name: &str, override_name: &str, alternate: &[&str]) -> Option<PathBuf> {
    if let Some(path) = env::var_os(override_name).map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }
    let names = std::iter::once(name).chain(alternate.iter().copied());
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        for candidate in names.clone().map(|name| directory.join(name)) {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    if cfg!(windows)
        && let Some(program_files) = env::var_os("ProgramFiles(x86)")
        && let Ok(versions) =
            std::fs::read_dir(Path::new(&program_files).join("Windows Kits/10/bin"))
    {
        for version in versions.flatten() {
            for architecture in ["x64", "x86"] {
                let candidate = version.path().join(architecture).join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn run_resource_tool(tool: &Path, args: &[&std::ffi::OsStr], directory: &Path) -> bool {
    Command::new(tool)
        .args(args)
        .current_dir(directory)
        .status()
        .is_ok_and(|status| status.success())
}

fn build_event_message_resource() {
    println!("cargo:rerun-if-changed=event_messages.mc");
    let target = env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    let mc_name = if target.ends_with("-msvc") {
        "mc.exe"
    } else {
        "windmc"
    };
    let rc_name = if target.ends_with("-msvc") {
        "rc.exe"
    } else {
        "windres"
    };
    let mc_alternates = if target.ends_with("-gnu") {
        &["x86_64-w64-mingw32-windmc"][..]
    } else {
        &[][..]
    };
    let rc_alternates = if target.ends_with("-gnu") {
        &["x86_64-w64-mingw32-windres"][..]
    } else {
        &[][..]
    };
    let Some(mc) = find_tool(mc_name, "NSSM_R_MC", mc_alternates) else {
        if target.ends_with("-msvc") {
            panic!("Windows SDK mc.exe is required to build the event message resource");
        }
        println!("cargo:warning=windmc not found; Windows event descriptions will be unavailable");
        return;
    };
    let Some(rc) = find_tool(rc_name, "NSSM_R_RC", rc_alternates) else {
        if target.ends_with("-msvc") {
            panic!("Windows SDK rc.exe is required to build the event message resource");
        }
        println!("cargo:warning=windres not found; Windows event descriptions will be unavailable");
        return;
    };
    let msvc = target.ends_with("-msvc");
    let (mc_input, mc_directory) = if msvc {
        let source = std::fs::read_to_string("event_messages.mc")
            .expect("read Windows event message definitions");
        let mut encoded = vec![0xff, 0xfe];
        encoded.extend(source.encode_utf16().flat_map(u16::to_le_bytes));
        let path = output.join("event_messages.mc");
        std::fs::write(&path, encoded).expect("write Unicode event message definitions");
        (
            std::ffi::OsString::from("event_messages.mc"),
            output.clone(),
        )
    } else {
        (
            std::ffi::OsString::from("event_messages.mc"),
            PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        )
    };
    let mut mc_args = Vec::<std::ffi::OsString>::new();
    if msvc {
        mc_args.push("-u".into());
    } else {
        mc_args.extend(["-C".into(), "65001".into()]);
    }
    mc_args.extend([
        "-h".into(),
        output.as_os_str().to_owned(),
        "-r".into(),
        output.as_os_str().to_owned(),
        mc_input,
    ]);
    let mc_args = mc_args
        .iter()
        .map(|arg| arg.as_os_str())
        .collect::<Vec<_>>();
    if !run_resource_tool(&mc, &mc_args, &mc_directory) {
        panic!("message compiler failed to build event_messages.mc");
    }

    let object = if msvc {
        output.join("event_messages.res")
    } else {
        output.join("event_messages.o")
    };
    let rc_args = if msvc {
        vec![
            std::ffi::OsString::from("/nologo"),
            std::ffi::OsString::from("/fo"),
            object.as_os_str().to_owned(),
            output.join("event_messages.rc").as_os_str().to_owned(),
        ]
    } else {
        vec![
            output.join("event_messages.rc").as_os_str().to_owned(),
            std::ffi::OsString::from("-O"),
            std::ffi::OsString::from("coff"),
            std::ffi::OsString::from("-o"),
            object.as_os_str().to_owned(),
        ]
    };
    let rc_args = rc_args
        .iter()
        .map(|arg| arg.as_os_str())
        .collect::<Vec<_>>();
    if !run_resource_tool(&rc, &rc_args, &output) {
        panic!("resource compiler failed to compile event_messages.rc");
    }
    println!("cargo:rustc-link-arg={}", object.display());
}

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let timestamp = std::env::var("SOURCE_DATE_EPOCH")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("SOURCE_DATE_EPOCH must be an unsigned Unix timestamp")
        })
        .unwrap_or_else(|_| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time must be after the Unix epoch")
                .as_secs()
        });
    println!(
        "cargo:rustc-env=NSSM_BUILD_DATE={}",
        build_date::from_unix_seconds(timestamp)
    );
    build_event_message_resource();
}
