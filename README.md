# nssm-rs

`nssm-rs` is an open source Rust implementation of the NSSM Windows service wrapper. It is a command-line tool only; there is no GUI installer.

[![CI](https://github.com/lihongjie0209/nssm-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/lihongjie0209/nssm-rs/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/lihongjie0209/nssm-rs)](https://github.com/lihongjie0209/nssm-rs/releases/latest)

## Download

The latest release is [v0.1.1](https://github.com/lihongjie0209/nssm-rs/releases/tag/v0.1.1). Download `nssm-rs-v0.1.1-windows-x64.zip` from the release page and extract `nssm-rs.exe`. A SHA-256 checksum file is provided alongside the archive.

## Quick start

Run an elevated PowerShell or Terminal for commands that create or manage Windows services. Replace the paths and service name with your own values:

```powershell
# Install a service that runs a program with arguments
.\nssm-rs.exe install ExampleService "C:\Apps\worker.exe" --config "C:\Apps\worker.toml"

# Inspect or change managed settings
.\nssm-rs.exe get ExampleService Application
.\nssm-rs.exe set ExampleService AppDirectory "C:\Apps"

# Control and inspect the service
.\nssm-rs.exe start ExampleService
.\nssm-rs.exe status ExampleService
.\nssm-rs.exe stop ExampleService

# Remove it after it has stopped
.\nssm-rs.exe remove ExampleService confirm
```

Run `nssm-rs.exe help` for the command list. The CLI supports `install`, `remove`, `start`, `stop`, `restart`, `pause`, `continue`, `status`, `statuscode`, `get`, `set`, `reset`, `unset`, `rotate`, `list`, `dump`, and `processes`.

## Features

- Create, configure, query, control, and remove Windows services through the Service Control Manager.
- Configure managed service settings in the Windows registry, including application arguments, working directory, service account, dependencies, environment, exit actions, restart delays, priorities, and processor affinity.
- Supervise child processes, apply restart and exit actions, and terminate child process trees when stopping a service.
- Run lifecycle hooks and redirect standard input, output, and error; support UTC line timestamps and log rotation.
- Inspect native service metadata, list services, dump configuration, and report process trees.

See [the compatibility plan](docs/COMPATIBILITY.md) and [reference comparison](docs/REFERENCE_COMPARISON.md) for the detailed feature inventory and acceptance criteria.

### Capability matrix

| Area | Status | Verification / notes |
| --- | --- | --- |
| CLI commands and Windows SCM lifecycle | Implemented | Install, remove, start, stop, restart, pause/continue, status and service listing are exercised by Windows integration tests. |
| Managed settings and native service configuration | Implemented | Registry-backed settings, service metadata, dependencies, account configuration, dump and reset operations have unit or integration coverage. |
| Child process supervision | Implemented | Exit actions, restart delays, throttling, process trees, affinity/priority, and stop methods are covered by Windows SCM scenarios. |
| Environment and lifecycle hooks | Implemented | Environment composition, start/stop/exit hooks, and failure cases are covered by integration tests; exact hook environment parity is still being compared. |
| Standard streams and logs | Implemented | Redirected stdin/stdout/stderr, timestamps, rotation, and setup failures are covered by Windows integration tests. |
| Unit and CLI tests | Passing | Run in GitHub Actions on Windows and Linux. |
| Windows SCM integration suite | Passing | Runs serially on `windows-latest`; includes a simple real service and end-to-end lifecycle checks. |
| Restart retry status parity | Known difference | During a 30-second failed-launch retry, nssm-rs reports `SERVICE_START_PENDING`; upstream reports `SERVICE_STOPPED`. The retry and child recovery are CI-tested. |
| Full NSSM command and output parity | In progress | See the detailed [reference comparison](docs/REFERENCE_COMPARISON.md) for remaining behavioral differences. |

## Build and tests

Build the CLI on Windows with Rust installed:

```powershell
cargo build --release
cargo test --lib --bins --test cli_compat --test cli_process
```

The command above runs the unit and CLI tests. Windows SCM integration tests create and remove real temporary services, so the GitHub Actions workflow runs the full suite serially on `windows-latest`. The cross-platform tests also run on Linux. The SCM suite requires an elevated Windows runner/account; CI does not open a GUI or UAC prompt.

See [GitHub Actions](https://github.com/lihongjie0209/nssm-rs/actions) for the latest run results.

## Compatibility notes

This is the first release, and compatibility work is ongoing. The [reference comparison](docs/REFERENCE_COMPARISON.md) tracks implemented behavior and remaining differences. One known difference is that nssm-rs reports `SERVICE_START_PENDING` during a 30-second retry after a failed restart launch; upstream NSSM reports `SERVICE_STOPPED` during that interval.

## Reference and license

The behavior reference is [ONLYOFFICE/nssm](https://github.com/ONLYOFFICE/nssm). This Rust implementation is independently written and does not copy upstream source files. NSSM's upstream documentation describes NSSM as public domain; see [NOTICE](NOTICE) and [LICENSE](LICENSE).
