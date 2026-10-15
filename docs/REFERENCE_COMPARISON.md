# NSSM reference comparison

This report records direct black-box checks against the pinned NSSM release.
It is a partial report, not the command-by-command release acceptance report.

## Reference and environment

- Source: `ONLYOFFICE/nssm` commit `f28f043161cf6d21c2bf5f923c23b1fbe74d236d`
  (tag `v2.24.1`).
- Reference executable: `nssm_x64.zip` from the
  [v2.24.1 GitHub release](https://github.com/ONLYOFFICE/nssm/releases/tag/v2.24.1).
- Downloaded archive SHA-256: `dd15b0bd3541f954220b8c0622a40294b919dfd0e6635859e733f153ba34bceb`.
- Both binaries were run on the same local Windows host on 2026-10-15.
- Output comparisons normalize trailing whitespace and, for service lists,
  sort service names before comparing.

## Checks completed

| Scenario | Result |
|---|---|
| `list all ignored` | Both exited 0 and returned the same 331 service names. This confirms the reference behavior of considering only the first optional argument. |
| `list definitely-not-all` | Both exited 0. The host had no NSSM-managed services, so both returned no names. |
| `list` and `list all` | Rechecked both commands against the pinned executable on 2026-10-15; output and exit status matched on this host. |
| `status Spooler` and `statuscode Spooler` | Output and status matched exactly. Both statuscode invocations returned the host's `SERVICE_RUNNING` numeric code (4). |
| `processes Spooler` when the service process cannot be opened | Both exited 0 and emitted no process-tree stdout. NSSM reports the process-open failure on stderr; nssm-rs now does the same rather than printing a fabricated `???` root. PowerShell remoting altered the reference's localized stderr encoding, so exact error-text bytes remain unverified. |
| `dump` for equivalent installed services | Compared complete output after normalizing the executable path and service name. Install line, Windows shell escaping, AppParameters, AppDirectory, default AppExit, extra environment, symbolic priority, rotation threshold, Description, DisplayName, ObjectName, Start and Type matched. |
| Managed/native setting set/get sweep | Set and read back AppNoConsole, AppPriority, AppRestartDelay, AppThrottle, AppRotateBytes, AppAffinity, AppExit Default, AppEnvironmentExtra, AppParameters, AppDirectory, Description, DisplayName, ObjectName, Start and Type on paired temporary services. Exit codes and normalized output matched for every case. |
| `get Spooler Start` | Both exited 0 and returned the same startup type. |
| `get Spooler Type` | Both exited 0 and returned the same service type. |
| `get Spooler DisplayName` | Both exited 0 and returned the same display name. |
| `get Spooler ObjectName` | Both exited 0 and returned the same account name. |
| `get Spooler DependOnService` | Both returned `RPCSS` and `http` in the same order. The reference emits `CR CR LF` between items and CRLF at the end; nssm-rs now reproduces those exact line-ending bytes for all CLI multi-string reads. |
| `set/get AppAffinity` | Directly compared on the same Windows host: `0-1` reads back as `0,1`; `0,1,2` reads back as `0-2`; `Default`, `All`, and `*` each read back as `All`. nssm-rs matches the reference output and exit status for these values. |
| `set/get AppAffinity 0-64` | Both binaries accept this range and read it back as `0-63` on the pinned x64 reference build. The reference parser checks the range start but not its end; nssm-rs reproduces the observed x64 result with explicit wrapping mask logic. A standalone CPU index `64` is rejected by both; both return exit code 6 and preserve the prior setting. |
| `get <new-service> AppExit Default` immediately after install / after first start | Both return `Restart`. NSSM creates the `AppExit` default during install; nssm-rs now does the same. A missing default inside an existing malformed key remains handled deterministically as `Restart` rather than NSSM's uninitialized control character. |
| `get Spooler Description` | Both binaries emitted the same UTF-8 Chinese text when captured as raw bytes. The reference used CRLF and nssm-rs initially used LF. Windows CLI output now uses CRLF; a Windows lifecycle integration assertion checks the raw bytes. |
| `statuscode` for a missing service | Both exited 0, matching NSSM's documented failure sentinel. Diagnostic wording differs. |
| Missing-service lookup for `status`, `start`, `stop`, `restart`, `pause`, `continue`, `rotate`, `get`, `set`, `reset`, and confirmed `remove` | Directly compared on the local Windows host: NSSM returns exit code 3 for each. nssm-rs now returns 3 and emits no stdout; a Windows integration test exercises the same command set. |
| Setting operation failures | The pinned source returns 5 when a known setting cannot be read and 6 when a known setting cannot be changed; unknown parameters return 1. nssm-rs maps these paths accordingly. The SCM lifecycle integration test checks the missing `AppExit` read (5), invalid `Start` value (6), and unknown setting (1). |
| `continue` during restart throttling | The reference writes Application event 1035 (`RESET_THROTTLE`) with the service name when Continue resets the throttle. nssm-rs now reports the same event from its SCM control handler; the real service integration test checks the event record. |
| SCM control diagnostics | The reference reports event 1040 (`SERVICE_CONTROL_HANDLED`) for handled START, STOP and CONTINUE controls and event 1041 (`SERVICE_CONTROL_NOT_HANDLED`) for unsupported PAUSE controls. nssm-rs now logs the same service/control insertion values; Windows SCM integration asserts START, STOP and PAUSE records. |
| Restart throttle event logging | The reference reports event 1034 (`THROTTLED`) with the configured threshold and retry delay, or event 1072 (`RESTART_DELAY`) when the configured restart delay exceeds exponential backoff. nssm-rs now emits both forms; SCM integration scenarios assert the event IDs and insertion values. |
| Successful log rotation | The reference reports event 1077 (`ROTATED`) with the service name, active log path, and backup path. nssm-rs now reports the same event for startup and live rotations; a Windows SCM integration assertion checks the event's three insertion values. |
| `version` | Both commands succeed, but each reports its own binary identity and build metadata. |

## Remaining comparison work

Run successful and failing black-box scenarios for every supported command,
setting, and relevant argument form. Compare output text and bytes, process exit
codes, service configuration, registry effects, event log records, and child
process behavior. Record each case here and resolve or explicitly document every
remaining difference before release acceptance.
