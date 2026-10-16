# NSSM CLI compatibility plan and acceptance criteria

This is the implementation contract for nssm-rs. Reference: the locally cloned
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
| SCM recovery behavior | Enable recovery actions for services that exit gracefully with a non-zero code; preserve service-configured recovery actions | Query `SERVICE_FAILURE_ACTIONS_FLAG` after install and verify its non-crash failure flag |
| Read/write/reset settings | `get`, `set`, `reset`; concatenate value arguments with spaces; defaults and invalid parameter handling | Table-driven tests for every setting; Windows round-trip of registry and native SCM values |
| Managed service settings | `Application`, `AppParameters`, `AppDirectory`, `AppExit`, `AppEvents`, `AppAffinity`, `AppEnvironment`, `AppEnvironmentExtra`, `AppNoConsole`, `AppPriority`, `AppRestartDelay`, `AppThrottle`, `AppKillProcessTree` | Parameter round-trip/default/validation tests; supervise real test child processes |
| I/O | `AppStdin`, `AppStdout`, `AppStderr`, their share mode, creation disposition and flags; stderr/stdout copy-and-truncate; append/truncate semantics | Child reads stdin and emits distinct streams; assert exact file contents and handle modes |
| Shutdown | `AppStopMethodSkip`, console Ctrl-C, window close, thread close and configurable grace periods; process-tree termination | Child fixture responds to each signal or ignores it; assert graceful path, timeout and descendants cleanup |
| Restart policy | Exit-code-specific/default `Restart`, `Ignore`, `Exit`, `Suicide`; throttle and restart delay; reset throttle after long-running success | Deterministic child fixtures exit with chosen codes and delays; assert restart counts, timing bounds, Ignore/Exit service states, and Suicide's abnormal SCM termination |
| Log rotation | `AppRotateFiles`, `AppRotateOnline`, `AppRotateSeconds`, low/high byte thresholds, delay, timestamping; `rotate <name>` | Boundary-size/age tests, active-file and naming assertions, integration for online rotation |
| Environment | App environment overrides, extra environment, system/service merged environment; ordered KEY=VALUE edits and case-insensitive keys; `+`, `-`, `:` modifiers and optional single-key query | Unit tests for ordering, replacement, conditional delete, clear/reset, malformed blocks and expansion; child sees expected environment |
| Service metadata | `Description`, `DisplayName`, `ObjectName`, `Start`, `Type`, `DependOnGroup`, `DependOnService`, `ImagePath`, `Name` | Native SCM readback, ImagePath update, Name write rejection, accepted enum validation, account/dependency error handling |
| Hooks | `AppEvents <event>/<action> <command>` for service/app start, stop, rotate lifecycle and pre/post phases; hook output-handle behavior and all `NSSM_*` ABI variables | Event-order integration assertions for normal start, restart, stop, rotation, failures, and cancellation; assert the full environment ABI on real Start, Exit, Stop and Rotate hooks |
| Discovery/reporting | `list` and `list all`; `processes <name>`; `dump <name>` and `dump <name> <newname>` | Golden output tests, quoting/escaping round-trip, process tree correctness, native-service filtering |
| Windows process launch | Correct Windows quoting, Unicode paths/args, environment expansion, priority, CPU affinity, console/window behavior | Launch fixture with argv/environment echo, Unicode and special-character cases, priority/affinity inspection |
| Event logging/errors | Windows Event Log diagnostics and correct system error reporting | Capture expected events/error codes for service start/stop and invalid SCM/registry operations; events 1001, 1005, 1006, 1008, 1010, 1011, 1012, 1013, 1014, 1015, 1016, 1017, 1019, 1020, 1021, 1028, 1032, 1033, 1034, 1035, 1038, 1039, 1040, 1041, 1045, 1046, 1047, 1048, 1052, 1053, 1060, 1064, 1065, 1066, 1067, 1068, 1069, 1070, 1072, 1073, 1077, 1078 and 1079 are verified |

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
settings. `Name` is query-only in the reference CLI; `ImagePath` can be set
through SCM. Each I/O
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
`status` reports a symbolic `SERVICE_*` state and exits successfully when the
query succeeds. `statuscode` prints the same symbolic state and returns the
numeric SCM state as its process exit code; query failure returns zero. `dump`
uses NSSM's own export behavior, including its omission of code-specific
`AppExit` actions and repeated default-action lines when multiple AppExit values
exist.

## Test plan

- Unit: parser arity/case handling; Windows argument quoting; setting codecs and
  defaults; environment block ordering/edit operators; dependency list operators;
  AppExit fallback; log rotation thresholds/naming; restart/throttle state machine;
  shutdown method ordering; hook event sequencing; error mapping.
- Integration (Windows): SCM lifecycle and metadata; registry persistence; real
  process launch/restart/stop; all three standard streams; descendants; rotation;
  hooks; account/dependency configuration where test privileges permit.
- Compatibility: run equivalent scenarios against the pinned NSSM reference and
  nssm-rs in a disposable Windows VM, compare service configuration, output,
  statuses, side effects and observable process behavior. Document intentional
  differences (ideally none for the stated CLI target).

Direct Windows reference checks are recorded in
[`REFERENCE_COMPARISON.md`](REFERENCE_COMPARISON.md). That report is partial and
does not satisfy the command-by-command release gate yet.

## Current status

Local Windows verification on 2026-10-16 used Windows 11 build 26200 from a
non-administrator session. The library's 17 tests, all 13 CLI compatibility
tests, all 6 CLI process tests and 43 Windows backend unit tests passed. One
backend case that installs a real service was filtered; an unfiltered run
reached it and SCM returned access denied. Event tests reused the existing NSSM
event source; failure cases for events 1017, 1019, 1028, 1032, 1033, 1038,
1039, 1060, 1064, 1065 and 1073 passed against the Windows Application log. A direct non-administrator
`install` invocation printed the administrator-required CLI diagnostic and returned 100
without launching UAC. The SCM integration suite still requires an elevated
Windows session and has not been run in full; four read-only/error-path cases
from it did pass against the live SCM. The pinned NSSM read-only comparison
matched all 106 safe invocations with exact exit codes and raw stdout/stderr on
2026-10-16;
the UAC-triggering missing-service `remove` case is intentionally excluded. The
Windows x64 release build also succeeded, and its `--version`, `help`, and
non-administrator `install` behaviors were checked on the host.

Command parsing, managed settings, AppEnvironment/AppEnvironmentExtra composition,
AppExit rules, priority/affinity, restart delay/throttle, SCM lifecycle, install,
remove, service listing, process launch with configured stdin/stdout/stderr files,
job-based process-tree termination and process-tree reporting are present. Native
process reporting checks that descendant PIDs were created during the service
process lifetime to avoid including unrelated processes after PID reuse.
metadata support now includes Description, DisplayName, ImagePath, ObjectName,
Start, Type, service Environment and service/group dependency lists. Direct
stdin/stdout/stderr file redirection, staged console/window/thread shutdown,
and service references by either SCM key name or display name are implemented;
the local reference comparison and Windows integration test check display-name
lookup and canonical-name output.
startup and online/on-demand log rotation, UTC per-line log timestamps,
Start/Pre, Start/Post, Stop/Pre, Exit/Post, Rotate and Power hooks, and
configuration dump are implemented. The Windows integration suite checks real
SCM install/start/stop/remove; process-tree reporting and descendant termination;
Start/Pre abort behavior; hook output redirection and event ordering; restart,
ignore and exit policies; environment composition; all three child standard
streams; and online/on-demand log rotation. The latest successful full Windows
SCM CI run is [run 38807996448](https://github.com/lihongjie0209/nssm-rs/actions/runs/38807996448)
for commit `0264806`; the current worktree adds further Windows tests that still
need a full run. CI runs the Windows unit/CLI suite and full SCM integration
suite as separate named steps. The standalone SCM smoke test also verifies
configuration readback, running state, wrapped process reporting, stop, removal
and service disappearance. A dedicated Windows settings integration test now
round-trips and resets all 28 managed DWORD settings against the real service
registry, including priority name normalization and each setting's default.
The stream logger now detects UTF-16LE output, writes a BOM to an empty log,
and encodes timestamps in the stream's character width; pure tests cover line
prefixing across chunks and Windows integration checks the resulting BOM and
decoded timestamp. This new Windows assertion still needs execution on Windows.
Service management commands do not launch UAC or any other GUI prompt. Commands
that require administrator access print a CLI diagnostic and exit; rerun them
from an elevated terminal. Windows unit coverage verifies which operations need
administrator access without launching a process.
Install now persists the application directory and `AppExit=Restart` defaults,
matching NSSM before the first service start. `dump` emits shell-safe commands,
symbolic priority values and native default metadata; the Windows lifecycle
integration replays the resulting commands into a second service.
The SCM lifecycle integration also changes a built-in account alias through
`ObjectName` and validates `Start` by reading back its real SCM value.
It exercises service dependencies through replacement, addition, removal and
reset against the SCM configuration, verifies that a service-dependency reset
clears service dependencies while preserving group dependencies, and verifies
that group-dependency reset leaves groups unchanged as NSSM does.
Another Windows integration test round-trips and resets Application,
AppParameters, AppDirectory, AppStdin, AppStdout and AppStderr, including a
Unicode directory and paths containing spaces. The environment integration
scenario also queries the raw AppEnvironment/AppEnvironmentExtra values, checks
that the child receives the expanded composed value, and verifies reset removes
each configured key.
Live log rotation closes and moves the active file
when CopyAndTruncate is disabled, or copies and truncates the same active file
when enabled; Windows integration tests check backup data, continued output and
active-file identity for both modes. The stream redirection test now exercises
both the default append behavior and explicit `CREATE_ALWAYS` truncation while
checking stdin, stdout and stderr contents. When stdout and stderr resolve to
the same Windows path, nssm-rs now duplicates the stdout handle, uses stdout's
open settings for both streams, and preserves NSSM's stdout logger / direct
stderr handle behavior for timestamping and online rotation; the Windows SCM
test suite verifies the stdout-wins behavior. The Windows suite also verifies that SCM stop reaches responding children through
CTRL+C, WM_CLOSE and WM_QUIT before termination fallback; hook tests capture all
documented `NSSM_*` ABI variables for Start/Pre, Exit/Post, Stop/Pre and
Rotate/Pre/Post and verify the configured working directory. Supported Power
events map to Change/Resume hooks and are dispatched from the SCM callback;
NSSM launches these hook processes asynchronously, and nssm-rs now does the
same. Windows unit tests cover event mapping, SCM control code, hook names and
async dispatch policy, plus the Power hook trigger, last-control name and
deadline values. A deterministic test checks the complete `NSSM_*` environment
mapping used by Power hooks, including the last application exit code when a
Power event arrives between child processes. Windows `ControlService` does not expose
`SERVICE_CONTROL_POWEREVENT` as a caller-sendable control, so injecting a real
power broadcast into a service still requires an OS power event; actual system
delivery remains unverified. The real SCM lifecycle smoke test now queries
`dwControlsAccepted` while the service is running and asserts that
`SERVICE_ACCEPT_POWEREVENT` is registered. A Windows test invokes the SCM callback for a
Resume event, launches the configured asynchronous hook, and checks the
`NSSM_EVENT`, `NSSM_ACTION`, `NSSM_TRIGGER`, `NSSM_LAST_CONTROL` and
`NSSM_EXITCODE` values received by the child. A service launch
integration test also copies the child fixture under a Unicode executable path,
passes Unicode arguments containing spaces, quotes, trailing backslashes, and an
empty argument, then checks the child's exact received argument list. Another
service integration test queries the child process after launch and verifies
that configured priority class and CPU affinity are effective at runtime. The
SCM lifecycle test also replays every command emitted by `dump` into a second
service and reads back its application, arguments, and service type. Remaining
acceptance scope includes system-level Power event delivery, broader error-path
coverage, and command-by-command parity against the pinned reference. Verified
event-log coverage includes events 1001, 1002, 1006, 1008, 1010, 1011, 1012,
1013, 1014, 1015, 1016, 1017, 1018, 1019, 1020, 1021, 1023, 1027, 1028,
1032, 1033, 1034, 1035, 1036, 1039, 1040, 1041, 1045, 1046, 1047, 1048,
1056, 1060, 1064, 1071, 1072, 1073 and 1077; events 1018, 1036, 1056 and 1071 cover invalid
numeric registry queries and default fallbacks. Missing-service exit status 3 is now
covered for status, start, stop,
restart, pause, continue, rotate, get, set, reset and confirmed remove. A real
SCM stop during the child-free restart-throttle wait verifies event 1012 and
its service/application insertion values; stopping a running child verifies
event 1011. The default Suicide/zero-exit integration asserts that event 1016
retains the original Suicide action and event 1021 explains the graceful fallback.
An inventory comparison against pinned upstream `messages.mc` found event 1003
without a matching nssm-rs diagnostic; its uses are allocation failures that
Rust's infallible allocation APIs generally abort on. Events 1059 and 1061 are
defined in the message catalog but are not referenced by the pinned upstream
source.
Event 1009 is not applicable: NSSM registers a `RegisterWaitForSingleObject`
callback, while nssm-rs monitors child exits with `Child::try_wait()` in its
service loop, so it has no corresponding registration failure.
Events 1022, 1037, 1054, 1055 and 1062 are also not applicable to the current
implementation: environment expansion is performed in Rust, delay waits use
`thread::sleep`, Win32 APIs are linked directly instead of resolved with
`LoadLibrary`/`GetProcAddress`, and child environments are passed at process
creation instead of being set in the service host.
Event 1039 is now covered: a wrong registry type for `AppEnvironment` or
`AppEnvironmentExtra` emits the upstream warning and ignores that value instead
of aborting service startup; a Windows event test verifies this behavior.
Event 1038 now identifies malformed `REG_MULTI_SZ` environment entries and names
the offending `AppEnvironment` or `AppEnvironmentExtra` value before startup is
rejected. A Windows backend test injects a malformed extra entry and verifies
the event's service, application and registry-value fields. A separate SCM
integration test verifies that this configuration stops actual service startup;
that lifecycle case still needs an elevated local run.
Event 1065 now follows NSSM's delayed-start fallback: a failed optional
`ChangeServiceConfig2W()` update leaves the service at ordinary automatic start,
logs the error except for unsupported-level errors, and keeps the setting
operation successful. Windows unit coverage checks the fallback error policy,
and injects a failure at the delayed-start configuration boundary to verify
event 1065 and successful fallback; the SCM integration round-trips delayed
and ordinary automatic start.
Event 1064 now records a service-description update failure while preserving
the command failure result. A Windows API failure injection verifies the event
and failure result against the Application log.
Event 1073 now reports explicit Windows log-pipe creation failure, then
continues with the log file as the child's direct output so the application can
still start. Windows tests verify successful inherited-pipe delivery, event
1073, and the direct-file fallback.
Event 1060 now reports logger-thread creation failure and falls back to direct
file output. A Windows failure-injection test verifies event 1060 and that the
child's output reaches the log file.
Events 1017 (required registry-key open failure), 1019 (`RegSetValueExW()` failure),
1028 (`TerminateProcess()` failure),
1032 (thread snapshot failure) and 1033 (thread enumeration failure) now have
deterministic Windows failure tests and passed locally against the Windows
Application log. A separate Windows API mock test covers console events
1045-1048, including the benign `ERROR_INVALID_HANDLE` and `ERROR_GEN_FAILURE`
AttachConsole results that NSSM suppresses.
Do not call this a complete NSSM replacement or publish a stable release until
each acceptance row has evidence.

The service-main unit test invokes `RegisterServiceCtrlHandlerExW` outside an
SCM dispatcher, producing the real Windows registration failure and verifying
event 1005 in the Application log. The read-only Windows reference sweep checks
all 39 managed setting names against a native service and compares exact
stdout/stderr bytes and exit status. The latest local sweep matched all 106
safe invocations against pinned NSSM v2.24.1 on 2026-10-16. An earlier 100-case
sweep passed in [GitHub Actions run
38787362624](https://github.com/lihongjie0209/nssm-rs/actions/runs/38787362624).
It verifies all nine NSSM version switch forms by success status and output
stream contract, plus missing-subparameter precedence for `AppExit` and
`AppEvents` across `get`, `reset`, `unset`, and safe `set` forms. The sweep
caught and resolved the extra carriage return NSSM emits for managed-setting
rejection and the process-open error's system message and trailing blank line;
Windows process/unit tests cover these cases.
The same reference workflow now runs a 22-operation disposable service
lifecycle comparison using `PING.EXE` as the child. Install, configuration
readback, Description set/reset, AppThrottle set/get/reset-to-default, start,
running status, repeated start, stop, stopped status, repeated stop and
confirmed removal match exit codes and raw stdout/stderr bytes against the
pinned executable. This verified NSSM's
already-running start failure and already-stopped stop success behavior, as
well as lifecycle success-message text and line endings.

When a child cannot be created after an application Restart action, upstream
logs error event 1010 with the service, executable and OS error, then event
1012 for the already-stopped process, event 1006 with service-specific exit
code 3, and warning event 1007. It reports
`SERVICE_STOPPED` during the 30-second wait, then tries the application again.
nssm-rs now follows this behavior; its former retry-state expectation was
incorrect. The paired Windows reference scenario checks all four events, the
stopped interval, and successful recovery, and awaits a green run.
Standard-stream creation failure now reports event 1049 with the file path and system error,
then event 1051, already-stopped event 1012, and start failure 1006 with the
output setup failure and service-specific exit code. Windows SCM tests check
these event records. Hook lookup and process creation failures now
report events 1081 and 1080 respectively; a Windows SCM scenario verifies a
missing Start/Pre executable does not abort service startup. A second Windows
SCM scenario denies SYSTEM read access to the isolated Start hook registry key
and verifies event 1081 without aborting startup. The last successful full
Windows SCM CI run on commit `0264806` passed the 1005, 1049/1051/1006,
1080/1081, 1029/1030, 1063, and identical stdout/stderr scenarios. A deterministic Windows API failure
injection now runs the process-tree reporting path and verifies that a new
Application log event 1024 is written and its Windows error is returned. A
second injection forces `Process32FirstW` to fail and verifies event 1025 in the
Application log. A disposable descendant process with injected `OpenProcess`
failure verifies event 1026; injected `GetProcessTimes` failure on the current
process verifies event 1044. An injected handle-duplication failure exercises
the shared error handler and verifies event 1050.

Missing `AppParameters` now logs warning 1029 and uses an empty argument list.
Missing or empty `AppDirectory` logs warning 1030 and falls back to the
application's parent directory, then the Windows directory. The Windows SCM
test checks event insertion values, the fallback directory, and child lifecycle.
