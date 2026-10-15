# nssm-rs

An independent Rust reimplementation of the command-line portion of NSSM, the Windows service wrapper. The project does not include a graphical installer.

> **Status: early development.** Argument parsing, Windows SCM control, install/remove, registry-backed managed settings, service hosting, environment composition, exit actions, restart throttling, process-tree termination, service listing, process-tree reporting, native service metadata editing, service dependency configuration, standard stream redirection, per-line UTC log timestamps, live move or copy-and-truncate rotation, staged console/window/thread shutdown, lifecycle/power hooks and configuration dump are implemented. Complete hook environment compatibility and command-by-command parity checks remain in progress. Do not use this build as a complete NSSM replacement or install production services with it.

## Build and run

```powershell
cargo build --release
cargo test
```

The binary is named `nssm-rs.exe`. On non-Windows hosts, `help` and `version` work; Windows SCM operations require Windows.

## Intended CLI compatibility

The target command set is `install`, `remove`, `start`, `stop`, `restart`, `pause`, `continue`, `status`, `statuscode`, `get`, `set`, `reset`/`unset`, `rotate`, `list`, `dump`, and `processes`, including NSSM-compatible service configuration and application supervision. GUI workflows are out of scope.

See [the compatibility plan](docs/COMPATIBILITY.md) for the full parameter inventory, implementation contract, and acceptance matrix.

## Reference and licensing

The behavior reference is [ONLYOFFICE/nssm](https://github.com/ONLYOFFICE/nssm), cloned locally as `../nssm-reference` during development. NSSM's upstream documentation describes NSSM as public domain; this Rust implementation is independently written and does not copy upstream source files. See [NOTICE](NOTICE) and [LICENSE](LICENSE).
