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

function Invoke-Cli([string] $Binary, [string[]] $Arguments) {
    $stdoutPath = [IO.Path]::GetTempFileName()
    $stderrPath = [IO.Path]::GetTempFileName()
    try {
        $argumentLine = ($Arguments | ForEach-Object { Quote-WindowsArgument $_ }) -join ' '
        $process = Start-Process -FilePath $Binary -ArgumentList $argumentLine -NoNewWindow -PassThru `
            -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
        if (-not $process.WaitForExit(20000)) {
            $process.Kill($true)
            $process.WaitForExit()
            throw "Timed out after 20 seconds: $Binary $argumentLine"
        }
        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            Stdout = [Convert]::ToBase64String([IO.File]::ReadAllBytes($stdoutPath))
            Stderr = [Convert]::ToBase64String([IO.File]::ReadAllBytes($stderrPath))
        }
    } finally {
        Remove-Item $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue
    }
}

function Add-Result([System.Collections.Generic.List[object]] $Results, [string] $Binary,
    [string] $Label, [string[]] $Arguments, [int[]] $AllowedExitCodes = @(0)) {
    $result = Invoke-Cli $Binary $Arguments
    if ($AllowedExitCodes -notcontains $result.ExitCode) {
        $stderr = [Text.Encoding]::Default.GetString([Convert]::FromBase64String($result.Stderr))
        throw "$Label returned $($result.ExitCode), expected $($AllowedExitCodes -join ', '): $stderr"
    }
    $Results.Add([pscustomobject]@{
        Label = $Label
        ExitCode = $result.ExitCode
        Stdout = $result.Stdout
        Stderr = $result.Stderr
    })
    return $result
}

function Wait-ServiceState([string] $Binary, [string] $Service, [string] $State) {
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        $result = Invoke-Cli $Binary @('status', $Service)
        if ($result.ExitCode -eq 0) {
            $stdout = [Text.Encoding]::Default.GetString([Convert]::FromBase64String($result.Stdout))
            if ($stdout.Trim() -eq $State) {
                return
            }
        }
        Start-Sleep -Milliseconds 100
    }
    throw "Service $Service did not reach $State"
}

function Run-Scenario([string] $Binary, [string] $Service, [string] $Application) {
    $results = [System.Collections.Generic.List[object]]::new()
    Add-Result $results $Binary 'install' @('install', $Service, $Application, '-t', '127.0.0.1') | Out-Null
    foreach ($setting in @('Application', 'AppParameters', 'AppDirectory', 'Start', 'Type')) {
        Add-Result $results $Binary "get $setting" @('get', $Service, $setting) | Out-Null
    }
    Add-Result $results $Binary 'get AppExit Default' @('get', $Service, 'AppExit', 'Default') | Out-Null
    Add-Result $results $Binary 'set Description' @('set', $Service, 'Description', 'nssm-rs parity fixture') | Out-Null
    Add-Result $results $Binary 'get Description' @('get', $Service, 'Description') | Out-Null
    Add-Result $results $Binary 'start' @('start', $Service) | Out-Null
    Wait-ServiceState $Binary $Service 'SERVICE_RUNNING'
    Add-Result $results $Binary 'status running' @('status', $Service) | Out-Null
    Add-Result $results $Binary 'stop' @('stop', $Service) | Out-Null
    Wait-ServiceState $Binary $Service 'SERVICE_STOPPED'
    Add-Result $results $Binary 'status stopped' @('status', $Service) | Out-Null
    Add-Result $results $Binary 'remove' @('remove', $Service, 'confirm') | Out-Null
    return ,$results.ToArray()
}

$stamp = [DateTime]::UtcNow.ToString('yyyyMMddHHmmssfff')
$service = "NssmRsParity$PID$stamp"
$application = Join-Path $env:WINDIR 'System32\PING.EXE'
$referenceResults = @()
$candidateResults = @()

try {
    $referenceResults = Run-Scenario $ReferenceBinary $service $application
    $candidateResults = Run-Scenario $CandidateBinary $service $application
} finally {
    foreach ($binary in @($ReferenceBinary, $CandidateBinary)) {
        Invoke-Cli $binary @('stop', $service) | Out-Null
        Invoke-Cli $binary @('remove', $service, 'confirm') | Out-Null
    }
}

if ($referenceResults.Count -ne $candidateResults.Count) {
    throw "Lifecycle result count differs: reference=$($referenceResults.Count), candidate=$($candidateResults.Count)"
}
for ($index = 0; $index -lt $referenceResults.Count; $index++) {
    $reference = $referenceResults[$index]
    $candidate = $candidateResults[$index]
    if ($reference.Label -ne $candidate.Label -or
        $reference.ExitCode -ne $candidate.ExitCode -or
        $reference.Stdout -cne $candidate.Stdout -or
        $reference.Stderr -cne $candidate.Stderr) {
        $referenceStdout = [Text.Encoding]::Default.GetString([Convert]::FromBase64String($reference.Stdout))
        $candidateStdout = [Text.Encoding]::Default.GetString([Convert]::FromBase64String($candidate.Stdout))
        $referenceStderr = [Text.Encoding]::Default.GetString([Convert]::FromBase64String($reference.Stderr))
        $candidateStderr = [Text.Encoding]::Default.GetString([Convert]::FromBase64String($candidate.Stderr))
        throw "Lifecycle mismatch at $($reference.Label): reference exit=$($reference.ExitCode), candidate exit=$($candidate.ExitCode)`nReference stdout: [$referenceStdout]`nCandidate stdout: [$candidateStdout]`nReference stderr: [$referenceStderr]`nCandidate stderr: [$candidateStderr]"
    }
    Write-Host "$($reference.Label): match (exit $($reference.ExitCode))"
}
Write-Host "Matched $($referenceResults.Count) lifecycle operations against the pinned NSSM reference."
