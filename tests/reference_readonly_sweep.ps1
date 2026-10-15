param(
    [Parameter(Mandatory = $true)]
    [string] $ReferenceBinary,

    [Parameter(Mandatory = $true)]
    [string] $CandidateBinary,
    [string] $OnlyCase
)

$ErrorActionPreference = 'Stop'

foreach ($binary in @($ReferenceBinary, $CandidateBinary)) {
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
        throw "Binary not found: $binary"
    }
}

# Only invoke recognized, non-interactive commands. In particular, do not run
# NSSM with no arguments: its detached-console fallback can open a GUI dialog.
$cases = @(
    @{ Name = 'help'; Args = @('help') },
    @{ Name = 'HELP'; Args = @('HELP') },
    @{ Name = '-h'; Args = @('-h') },
    @{ Name = '--help'; Args = @('--help') },
    @{ Name = 'list'; Args = @('list') },
    @{ Name = 'list all'; Args = @('list', 'all') },
    @{ Name = 'list all with ignored trailing argument'; Args = @('LIST', 'all', 'ignored') },
    @{ Name = 'get Spooler Start'; Args = @('get', 'Spooler', 'Start') },
    @{ Name = 'get Spooler Type'; Args = @('get', 'Spooler', 'Type') },
    @{ Name = 'get Spooler DisplayName'; Args = @('get', 'Spooler', 'DisplayName') },
    @{ Name = 'get Spooler ObjectName'; Args = @('get', 'Spooler', 'ObjectName') },
    @{ Name = 'get Spooler Name'; Args = @('get', 'Spooler', 'Name') },
    @{ Name = 'get Spooler ImagePath'; Args = @('get', 'Spooler', 'ImagePath') },
    @{ Name = 'get Spooler DependOnGroup'; Args = @('get', 'Spooler', 'DependOnGroup') },
    @{ Name = 'get Spooler Description'; Args = @('get', 'Spooler', 'Description') },
    @{ Name = 'get Spooler Environment'; Args = @('get', 'Spooler', 'Environment') },
    @{ Name = 'get Schedule Start'; Args = @('get', 'Schedule', 'Start') },
    @{ Name = 'get Schedule Type'; Args = @('get', 'Schedule', 'Type') },
    @{ Name = 'get Schedule Type with ignored trailing argument'; Args = @('get', 'Schedule', 'Type', 'ignored') },
    @{ Name = 'get Schedule Name'; Args = @('get', 'Schedule', 'Name') },
    @{ Name = 'get Schedule ImagePath'; Args = @('get', 'Schedule', 'ImagePath') },
    @{ Name = 'get Schedule DisplayName'; Args = @('get', 'Schedule', 'DisplayName') },
    @{ Name = 'get Schedule ObjectName'; Args = @('get', 'Schedule', 'ObjectName') },
    @{ Name = 'get Schedule DependOnGroup'; Args = @('get', 'Schedule', 'DependOnGroup') },
    @{ Name = 'get Schedule Description'; Args = @('get', 'Schedule', 'Description') },
    @{ Name = 'get Schedule Environment'; Args = @('get', 'Schedule', 'Environment') },
    @{ Name = 'get Schedule DependOnService'; Args = @('get', 'Schedule', 'DependOnService') },
    @{ Name = 'get Task Scheduler Start by display name'; Args = @('get', 'Task Scheduler', 'Start') },
    @{ Name = 'get Task Scheduler Name by display name'; Args = @('get', 'Task Scheduler', 'Name') },
    @{ Name = 'get Spooler DependOnService'; Args = @('get', 'Spooler', 'DependOnService') },
    @{ Name = 'get missing service'; Args = @('get', 'NssmRDefinitelyMissingService', 'Start') },
    @{ Name = 'get unknown setting on missing service'; Args = @('get', 'NssmRDefinitelyMissingService', 'NssmRDefinitelyNotASetting') },
    @{ Name = 'set on missing service'; Args = @('set', 'NssmRDefinitelyMissingService', 'Start', '3') },
    @{ Name = 'set unknown setting on missing service'; Args = @('set', 'NssmRDefinitelyMissingService', 'NssmRDefinitelyNotASetting', 'value') },
    @{ Name = 'reset on missing service'; Args = @('reset', 'NssmRDefinitelyMissingService', 'Start') },
    @{ Name = 'reset unknown setting on missing service'; Args = @('reset', 'NssmRDefinitelyMissingService', 'NssmRDefinitelyNotASetting') },
    @{ Name = 'unset on missing service'; Args = @('unset', 'NssmRDefinitelyMissingService', 'AppParameters') },
    @{ Name = 'unset unknown setting on missing service'; Args = @('unset', 'NssmRDefinitelyMissingService', 'NssmRDefinitelyNotASetting') },
    @{ Name = 'case-insensitive GET'; Args = @('GET', 'Spooler', 'Start') },
    @{ Name = 'status Spooler'; Args = @('STATUS', 'Spooler') },
    @{ Name = 'statuscode Spooler'; Args = @('STATUSCODE', 'Spooler') },
    @{ Name = 'status Schedule'; Args = @('status', 'Schedule') },
    @{ Name = 'statuscode Schedule'; Args = @('statuscode', 'Schedule') },
    @{ Name = 'dump Schedule with output name'; Args = @('dump', 'Schedule', 'NssmRsDumpCopy') },
    @{ Name = 'processes Spooler'; Args = @('processes', 'Spooler') },
    @{ Name = 'processes missing service and Spooler'; Args = @('processes', 'NssmRDefinitelyMissingService', 'Spooler') },
    @{ Name = 'status missing service'; Args = @('status', 'NssmRDefinitelyMissingService') },
    @{ Name = 'unknown setting on native service'; Args = @('get', 'Spooler', 'DefinitelyNotASetting') }
)

# Version output names each executable, so compare its command contract instead
# of requiring the reference and candidate identity strings to match.
foreach ($versionSwitch in @(
    'version', '/version', '/VERSION', '-v', '-V', '-version', '-VERSION', '--version', '--VERSION'
)) {
    $cases += @{ Name = "version switch $versionSwitch"; Args = @($versionSwitch); VersionCommand = $true }
}

# Query every managed setting against a native service. This exercises NSSM's
# per-setting native-service rejection path without changing machine state.
$managedSettings = @(
    'Application', 'AppParameters', 'AppDirectory', 'AppExit', 'AppEvents',
    'AppAffinity', 'AppEnvironment', 'AppEnvironmentExtra', 'AppNoConsole',
    'AppPriority', 'AppRestartDelay', 'AppStdin', 'AppStdinShareMode',
    'AppStdinCreationDisposition', 'AppStdinFlagsAndAttributes', 'AppStdout',
    'AppStdoutShareMode', 'AppStdoutCreationDisposition', 'AppStdoutFlagsAndAttributes',
    'AppStdoutCopyAndTruncate', 'AppStderr', 'AppStderrShareMode',
    'AppStderrCreationDisposition', 'AppStderrFlagsAndAttributes',
    'AppStderrCopyAndTruncate', 'AppStopMethodSkip', 'AppStopMethodConsole',
    'AppStopMethodWindow', 'AppStopMethodThreads', 'AppKillProcessTree',
    'AppThrottle', 'AppRedirectHook', 'AppRotateFiles', 'AppRotateOnline',
    'AppRotateSeconds', 'AppRotateBytes', 'AppRotateBytesHigh', 'AppRotateDelay',
    'AppTimestampLog'
)
foreach ($setting in $managedSettings) {
    $cases += @{ Name = "get Spooler $setting (native service)"; Args = @('get', 'Spooler', $setting) }
}
if ($OnlyCase) {
    $cases = @($cases | Where-Object { $_.Name -eq $OnlyCase })
    if ($cases.Count -eq 0) {
        throw "Unknown case: $OnlyCase"
    }
}

function Quote-WindowsArgument([string] $Argument) {
    if ($Argument.Length -gt 0 -and $Argument -notmatch '[\s"]') {
        return $Argument
    }
    $quoted = '"'
    $slashes = 0
    foreach ($character in $Argument.ToCharArray()) {
        if ($character -eq '\') {
            $slashes++
            continue
        }
        if ($character -eq '"') {
            $quoted += ('\' * (2 * $slashes + 1)) + '"'
        } else {
            $quoted += ('\' * $slashes) + $character
        }
        $slashes = 0
    }
    return $quoted + ('\' * (2 * $slashes)) + '"'
}

function Invoke-ReadOnlyCase([string] $Binary, [string[]] $Arguments) {
    $stdoutPath = [IO.Path]::GetTempFileName()
    $stderrPath = [IO.Path]::GetTempFileName()
    $argumentLine = ($Arguments | ForEach-Object { Quote-WindowsArgument $_ }) -join ' '
    $process = Start-Process -FilePath $Binary -ArgumentList $argumentLine -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
    $stdout = [Convert]::ToBase64String([IO.File]::ReadAllBytes($stdoutPath))
    $stderr = [Convert]::ToBase64String([IO.File]::ReadAllBytes($stderrPath))
    Remove-Item $stdoutPath, $stderrPath
    return [pscustomobject]@{
        ExitCode = $process.ExitCode
        Stdout = $stdout
        Stderr = $stderr
    }
}

$differences = 0
foreach ($case in $cases) {
    $reference = Invoke-ReadOnlyCase $ReferenceBinary $case.Args
    $candidate = Invoke-ReadOnlyCase $CandidateBinary $case.Args
    if ($case.VersionCommand) {
        $matches = $reference.ExitCode -eq 0 -and $candidate.ExitCode -eq 0 -and
            $reference.Stdout -and $candidate.Stdout -and
            -not $reference.Stderr -and -not $candidate.Stderr
    } else {
        $matches = $reference.ExitCode -eq $candidate.ExitCode -and
            $reference.Stdout -ceq $candidate.Stdout -and
            $reference.Stderr -ceq $candidate.Stderr
    }
    '{0}: {1} (exit {2}/{3})' -f $case.Name, $(if ($matches) { 'match' } else { 'DIFFERENCE' }), $reference.ExitCode, $candidate.ExitCode
    if (-not $matches) {
        $differences++
        "  reference stdout: $([Text.Encoding]::Default.GetString([Convert]::FromBase64String($reference.Stdout)))"
        "  candidate stdout: $([Text.Encoding]::Default.GetString([Convert]::FromBase64String($candidate.Stdout)))"
        "  reference stderr: $([Text.Encoding]::Default.GetString([Convert]::FromBase64String($reference.Stderr)))"
        "  candidate stderr: $([Text.Encoding]::Default.GetString([Convert]::FromBase64String($candidate.Stderr)))"
        "  reference stderr hex: $([BitConverter]::ToString([Convert]::FromBase64String($reference.Stderr)))"
        "  candidate stderr hex: $([BitConverter]::ToString([Convert]::FromBase64String($candidate.Stderr)))"
    }
}

if ($differences -gt 0) {
    throw "$differences reference comparison(s) differed"
}
