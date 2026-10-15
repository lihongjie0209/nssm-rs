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
| `get Spooler Start` | Both exited 0 and returned the same startup type. |
| `get Spooler Type` | Both exited 0 and returned the same service type. |
| `get Spooler DisplayName` | Both exited 0 and returned the same display name. |
| `get Spooler ObjectName` | Both exited 0 and returned the same account name. |
| `get Spooler DependOnService` | Both returned `RPCSS` and `http` in the same order. The reference emits `CR CR LF` between items and CRLF at the end; nssm-r emits standard CRLF between items and LF at the end. |
| `set/get AppAffinity` | Directly compared on the same Windows host: `0-1` reads back as `0,1`; `0,1,2` reads back as `0-2`; `Default`, `All`, and `*` each read back as `All`. nssm-r matches the reference output and exit status for these values. |
| `set/get AppAffinity 0-64` | Both binaries accept this range and read it back as `0-63` on the pinned x64 reference build. The reference parser checks the range start but not its end; nssm-r reproduces the observed x64 result with explicit wrapping mask logic. A standalone CPU index `64` is rejected by both; both return exit code 6 and preserve the prior setting. |
| `get <new-service> AppExit Default` before first start / after first start | Both fail before the first start because the `AppExit` key does not yet exist, then return `Restart` after startup initializes the default rule. NSSM emits an uninitialized control character when the key exists but has no default value; nssm-r keeps a deterministic `Restart` fallback for this malformed state. |
| `get Spooler Description` | Both binaries emitted the same UTF-8 Chinese text when captured as raw bytes. The reference used CRLF and nssm-r initially used LF. Windows CLI output now uses CRLF; a Windows lifecycle integration assertion checks the raw bytes. |
| `statuscode` for a missing service | Both exited 0, matching NSSM's documented failure sentinel. Diagnostic wording differs. |
| `version` | Both commands succeed, but each reports its own binary identity and build metadata. |

## Remaining comparison work

Run successful and failing black-box scenarios for every supported command,
setting, and relevant argument form. Compare output text and bytes, process exit
codes, service configuration, registry effects, event log records, and child
process behavior. Record each case here and resolve or explicitly document every
remaining difference before release acceptance.
