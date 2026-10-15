param(
    [Parameter(Mandatory = $true)]
    [string] $ReferenceBinary,

    [Parameter(Mandatory = $true)]
    [string] $CandidateBinary
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
    @{ Name = 'list'; Args = @('list') },
    @{ Name = 'list all'; Args = @('list', 'all') },
    @{ Name = 'list all with ignored trailing argument'; Args = @('LIST', 'all', 'ignored') },
    @{ Name = 'get Spooler Start'; Args = @('get', 'Spooler', 'Start') },
    @{ Name = 'get Spooler Type'; Args = @('get', 'Spooler', 'Type') },
    @{ Name = 'get Spooler DisplayName'; Args = @('get', 'Spooler', 'DisplayName') },
    @{ Name = 'get Spooler ObjectName'; Args = @('get', 'Spooler', 'ObjectName') },
    @{ Name = 'get Schedule Start'; Args = @('get', 'Schedule', 'Start') },
    @{ Name = 'get Schedule Type'; Args = @('get', 'Schedule', 'Type') },
    @{ Name = 'get Schedule Type with ignored trailing argument'; Args = @('get', 'Schedule', 'Type', 'ignored') },
    @{ Name = 'case-insensitive GET'; Args = @('GET', 'Spooler', 'Start') },
    @{ Name = 'status Spooler'; Args = @('STATUS', 'Spooler') },
    @{ Name = 'statuscode Spooler'; Args = @('STATUSCODE', 'Spooler') }
)

function Invoke-ReadOnlyCase([string] $Binary, [string[]] $Arguments) {
    $output = @(& $Binary @Arguments 2>&1 | ForEach-Object { [string] $_ })
    $exitCode = $LASTEXITCODE
    return [pscustomobject]@{
        ExitCode = $exitCode
        Output = $output -join "`n"
    }
}

$differences = 0
foreach ($case in $cases) {
    $reference = Invoke-ReadOnlyCase $ReferenceBinary $case.Args
    $candidate = Invoke-ReadOnlyCase $CandidateBinary $case.Args
    $matches = $reference.ExitCode -eq $candidate.ExitCode -and
        $reference.Output -ceq $candidate.Output
    '{0}: {1} (exit {2}/{3})' -f $case.Name, $(if ($matches) { 'match' } else { 'DIFFERENCE' }), $reference.ExitCode, $candidate.ExitCode
    if (-not $matches) {
        $differences++
        "  reference: $($reference.Output)"
        "  candidate: $($candidate.Output)"
    }
}

if ($differences -gt 0) {
    throw "$differences reference comparison(s) differed"
}
