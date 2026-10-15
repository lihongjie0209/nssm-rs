# NSSM CLI compatibility plan and acceptance criteria

This is the implementation contract for nssm-r. Reference: the locally cloned
[`ONLYOFFICE/nssm`](https://github.com/ONLYOFFICE/nssm) tree at commit
`f28f043161cf6d21c2bf5f923c23b1fbe74d236d`. The target is its command-line
interface and Windows service behavior. GUI workflows are explicitly excluded.

## Global acceptance rules

1. Every command and parameter below must work on supported Windows versions and
   match NSSM's documented argument handling, output meaning, exit status and
   persistent service state. Unsupported host platforms must fail clearly.
2. Every command parser branch and configuration value rule has unit coverage.
   Windows integration tests use an isolated temporary service and disposable
   child processes; they verify SCM/registry state before and after each action.
3. Integration coverage includes success, invalid input, missing permissions,
   missing service, already-running/stopped service, cancellation and cleanup.
   Tests must not depend on an external network or a pre-existing machine service.
4. Release acceptance requires all automated tests passing on Windows x64, a
   clean release build, and a command-by-command compatibility report. No command
   may silently claim success when its platform operation failed.

## CLI feature inventory

| Area | Required behavior | Acceptance evidence |
|---|---|---|
| Help/version | No-argument usage, `help`, version query | Snapshot/exit-code tests; matches binary version |
| Install | Interactive-less `install <name> <application> [args...]`; install service wrapper and store executable, arguments, working directory and defaults | Windows integration creates service, reads back config, starts/stops child, removes service |
| Remove | `remove <name>` confirmation flow and `remove <name> confirm`; wait/handle SCM deletion state | TTY/non-TTY prompt tests; integration confirms service disappears |
| SCM control | `start`, `stop`, `restart`, `status`, `statuscode` | Verify exact resulting SCM state and documented text/numeric result and exit codes |
| Read/write/reset settings | `get`, `set`, `reset`; concatenate value arguments with spaces; defaults and invalid parameter handling | Table-driven tests for every setting; Windows round-trip of registry and native SCM values |
| Managed service settings | `Application`, `AppParameters`, `AppDirectory`, `AppExit`, `AppEvents`, `AppAffinity`, `AppEnvironment`, `AppEnvironmentExtra`, `AppNoConsole`, `AppPriority`, `AppRestartDelay`, `AppThrottle`, `AppKillProcessTree` | Parameter round-trip/default/validation tests; supervise real test child processes |
| I/O | `AppStdin`, `AppStdout`, `AppStderr`, their share mode, creation disposition and flags; stderr/stdout copy-and-truncate; append/truncate semantics | Child reads stdin and emits distinct streams; assert exact file contents and handle modes |
| Shutdown | `AppStopMethodSkip`, console Ctrl-C, window close, thread close and configurable grace periods; process-tree termination | Child fixture responds to each signal or ignores it; assert graceful path, timeout and descendants cleanup |
| Restart policy | Exit-code-specific/default `Restart`, `Ignore`, `Exit`, `Suicide`; throttle and restart delay; reset throttle after long-running success | Deterministic child fixtures exit with chosen codes and delays; assert restart counts, timing bounds and stop state |
| Log rotation | `AppRotateFiles`, `AppRotateOnline`, `AppRotateSeconds`, low/high byte thresholds, delay, timestamping; `rotate <name>` | Boundary-size/age tests, active-file and naming assertions, integration for online rotation |
| Environment | App environment overrides, extra environment, system/service merged environment; ordered KEY=VALUE edits and case-insensitive keys; `+`, `-`, `:` modifiers and optional single-key query | Unit tests for ordering, replacement, conditional delete, clear/reset, malformed blocks and expansion; child sees expected environment |
| Service metadata | `Description`, `DisplayName`, `ObjectName`, `Start`, `Type`, `DependOnGroup`, `DependOnService`, `ImagePath`, `Name` | Native SCM readback, accepted enum validation, account/dependency error handling |
| Hooks | `AppEvents <event>/<action> <command>` for service/app start, stop, rotate lifecycle and pre/post phases; hook output-handle behavior and all `NSSM_*` ABI variables | Event-order integration assertions for normal start, restart, stop, rotation, failures, and cancellation; assert the full environment ABI on real Start, Exit, Stop and Rotate hooks |
| Discovery/reporting | `list` and `list all`; `processes <name>`; `dump <name>` and `dump <name> <newname>` | Golden output tests, quoting/escaping round-trip, process tree correctness, native-service filtering |
| Windows process launch | Correct Windows quoting, Unicode paths/args, environment expansion, priority, CPU affinity, console/window behavior | Launch fixture with argv/environment echo, Unicode and special-character cases, priority/affinity inspection |
| Event logging/errors | Windows Event Log diagnostics and correct system error reporting | Capture expected events/error codes for service start/stop and invalid SCM/registry operations; currently events 1001 (dispatcher failure), 1006 (service start failure), 1008 (child started), and 1010 (child launch failure) are verified |

### Complete setting-name inventory

The following names are individually in scope for `get`, `set`, `reset`,
configuration dump where supported, default-value handling and validation:

```text
Application, AppParameters, AppDirectory, AppExit, AppEvents, AppAffinity,
AppEnvironment, AppEnvironmentExtra, AppNoConsole, AppPriority,
AppRestartDelay, AppStdin, AppStdinShareMode, AppStdinCreationDisposition,
AppStdinFlagsAndAttributes, AppStdout, AppStdoutShareMode,
AppStdoutCreationDisposition, AppStdoutFlagsAndAttributes,
AppStdoutCopyAndTruncate, AppStderr, AppStderrShareMode,
AppStderrCreationDisposition, AppStderrFlagsAndAttributes,
AppStderrCopyAndTruncate, AppStopMethodSkip, AppStopMethodConsole,
AppStopMethodWindow, AppStopMethodThreads, AppKillProcessTree, AppThrottle,
AppRedirectHook, AppRotateFiles, AppRotateOnline, AppRotateSeconds,
AppRotateBytes, AppRotateBytesHigh, AppRotateDelay, AppTimestampLog,
DependOnGroup, DependOnService, Description, DisplayName, Environment,
ImagePath, ObjectName, Name, Start, Type
```

`DependOnGroup`, `DependOnService`, `Description`, `DisplayName`, `Environment`,
`ImagePath`, `ObjectName`, `Name`, `Start` and `Type` are native SCM/service-key
settings. `Name` and `ImagePath` are query-only in the reference CLI. Each I/O
stream's `ShareMode`, `CreationDisposition` and `FlagsAndAttributes` are separate
settings formed by appending those suffixes to `AppStdin`, `AppStdout` or
`AppStderr`.

`AppParameters` is the registry spelling in the source; the README's prose also
refers to the application arguments as options. The final CLI must preserve NSSM's
actual names and aliases as established by black-box comparison.

## Commands and output contract

Required command set: `install`, `remove`, `start`, `stop`, `restart`, `pause`,
`continue`, `status`, `statuscode`, `get`, `set`, `reset`/`unset`, `rotate`,
`list`, `dump`, `processes`. `edit` is excluded because it is a GUI workflow.
`list all` includes services not managed by NSSM where upstream does so. `dump`
must emit valid, safely quoted command lines that can recreate the configuration.
`status` reports a symbolic `SERVICE_*` state and success/failure exit status;
`statuscode` reports the numeric SCM state and returns nonzero on query failure.

## Test plan

- Unit: parser arity/case handling; Windows argument quoting; setting codecs and
  defaults; environment block ordering/edit operators; dependency list operators;
  AppExit fallback; log rotation thresholds/naming; restart/throttle state machine;
  shutdown method ordering; hook event sequencing; error mapping.
- Integration (Windows): SCM lifecycle and metadata; registry persistence; real
  process launch/restart/stop; all three standard streams; descendants; rotation;
  hooks; account/dependency configuration where test privileges permit.
- Compatibility: run equivalent scenarios against the pinned NSSM reference and
  nssm-r in a disposable Windows VM, compare service configuration, output,
  statuses, side effects and observable process behavior. Document intentional
  differences (ideally none for the stated CLI target).

## Current status

Command parsing, managed settings, AppEnvironment/AppEnvironmentExtra composition,
AppExit rules, priority/affinity, restart delay/throttle, SCM lifecycle, install,
remove, service listing, process launch with configured stdin/stdout/stderr files,
job-based process-tree termination and process-tree reporting are present. Native
metadata support now includes Description, DisplayName, ImagePath, ObjectName,
Start, Type, service Environment and service/group dependency lists. Direct
stdin/stdout/stderr file redirection, staged console/window/thread shutdown,
startup and online/on-demand log rotation, UTC per-line log timestamps,
Start/Pre, Start/Post, Stop/Pre, Exit/Post, Rotate and Power hooks, and
configuration dump are implemented. The Windows integration suite checks real
SCM install/start/stop/remove; process-tree reporting and descendant termination;
Start/Pre abort behavior; hook output redirection and event ordering; restart,
ignore and exit policies; environment composition; all three child standard
streams; and online/on-demand log rotation. It passes locally on Windows and in
GitHub Actions. CI runs the Windows unit/CLI suite and the full SCM integration
suite as separate named steps. The standalone SCM smoke test also verifies
configuration readback, running state, wrapped process reporting, stop, removal
and service disappearance. Live log rotation closes and moves the active file
when CopyAndTruncate is disabled, or copies and truncates the same active file
when enabled; Windows integration tests check backup data, continued output and
active-file identity for both modes. The Windows suite also verifies that SCM stop reaches responding children through
CTRL+C, WM_CLOSE and WM_QUIT before termination fallback; hook tests capture all
documented `NSSM_*` ABI variables for Start/Pre, Exit/Post, Stop/Pre and
Rotate/Pre/Post and verify the configured working directory. Remaining
acceptance scope includes hook ABI context parity for Power events, event-log compatibility
beyond events 1001, 1006, 1008, and 1010, broader error-path coverage, and
command-by-command parity against the pinned reference.
Do not call this a complete NSSM replacement or publish a stable release until
each acceptance row has evidence.
