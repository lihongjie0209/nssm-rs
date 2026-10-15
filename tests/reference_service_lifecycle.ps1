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

function Assert-SettingValue($Result, [string] $Expected, [string] $Label) {
    $actual = [Text.Encoding]::Default.GetString([Convert]::FromBase64String($Result.Stdout)).Trim()
    if ($actual -cne $Expected) {
        throw "$Label returned [$actual], expected [$Expected]"
    }
}

function Compare-StringSetting([System.Collections.Generic.List[object]] $Results,
    [string] $Binary, [string] $Service, [string] $Name, [string] $Value,
    [object] $Default, [string] $Subparameter = '', [switch] $SkipReset) {
    $setArguments = @('set', $Service, $Name)
    $getArguments = @('get', $Service, $Name)
    $resetArguments = @('reset', $Service, $Name)
    if ($Subparameter) {
        $setArguments += $Subparameter
        $getArguments += $Subparameter
        $resetArguments += $Subparameter
    }
    $setArguments += $Value
    Add-Result $Results $Binary "set $Name" $setArguments | Out-Null
    $configured = Add-Result $Results $Binary "get $Name" $getArguments
    if ($null -ne $Default) {
        Assert-SettingValue $configured $Value "get $Name"
    }
    if ($SkipReset) {
        Write-Host "Skipped reference reset $Name because pinned NSSM v2.24.1 crashes on this operation."
        return
    }
    Add-Result $Results $Binary "reset $Name" $resetArguments | Out-Null
    $reset = Add-Result $Results $Binary "get reset $Name" $getArguments
    if ($null -ne $Default) {
        Assert-SettingValue $reset $Default "get reset $Name"
    }
}

function Run-Scenario([string] $Binary, [string] $Service, [string] $Application) {
    $results = [System.Collections.Generic.List[object]]::new()
    Add-Result $results $Binary 'install' @('install', $Service, $Application, '-t', '127.0.0.1') | Out-Null
    foreach ($setting in @('Application', 'AppParameters', 'AppDirectory', 'Start', 'Type')) {
        Add-Result $results $Binary "get $setting" @('get', $Service, $setting) | Out-Null
    }
    Add-Result $results $Binary 'get AppExit Default' @('get', $Service, 'AppExit', 'Default') | Out-Null
    Add-Result $results $Binary 'set Description' @('set', $Service, 'dEsCrIpTiOn', 'nssm-rs parity fixture') | Out-Null
    Add-Result $results $Binary 'get Description' @('get', $Service, 'Description') | Out-Null
    Add-Result $results $Binary 'reset Description' @('reset', $Service, 'Description') | Out-Null
    Add-Result $results $Binary 'get reset Description' @('get', $Service, 'Description') | Out-Null
    $dwordSettings = @(
        @{ Name = 'AppNoConsole'; Value = '1'; Expected = '1'; Default = '0' },
        @{ Name = 'AppPriority'; Value = 'HIGH_PRIORITY_CLASS'; Expected = 'HIGH_PRIORITY_CLASS'; Default = 'NORMAL_PRIORITY_CLASS' },
        @{ Name = 'AppRestartDelay'; Value = '321'; Expected = '321'; Default = '0' },
        @{ Name = 'AppStdinShareMode'; Value = '3'; Expected = '3'; Default = '2' },
        @{ Name = 'AppStdinCreationDisposition'; Value = '2'; Expected = '2'; Default = '3' },
        @{ Name = 'AppStdinFlagsAndAttributes'; Value = '256'; Expected = '256'; Default = '128' },
        @{ Name = 'AppStdoutShareMode'; Value = '2'; Expected = '2'; Default = '3' },
        @{ Name = 'AppStdoutCreationDisposition'; Value = '3'; Expected = '3'; Default = '4' },
        @{ Name = 'AppStdoutFlagsAndAttributes'; Value = '256'; Expected = '256'; Default = '128' },
        @{ Name = 'AppStdoutCopyAndTruncate'; Value = '1'; Expected = '1'; Default = '0' },
        @{ Name = 'AppStderrShareMode'; Value = '2'; Expected = '2'; Default = '3' },
        @{ Name = 'AppStderrCreationDisposition'; Value = '3'; Expected = '3'; Default = '4' },
        @{ Name = 'AppStderrFlagsAndAttributes'; Value = '256'; Expected = '256'; Default = '128' },
        @{ Name = 'AppStderrCopyAndTruncate'; Value = '1'; Expected = '1'; Default = '0' },
        @{ Name = 'AppStopMethodSkip'; Value = '7'; Expected = '7'; Default = '0' },
        @{ Name = 'AppStopMethodConsole'; Value = '321'; Expected = '321'; Default = '1500' },
        @{ Name = 'AppStopMethodWindow'; Value = '321'; Expected = '321'; Default = '1500' },
        @{ Name = 'AppStopMethodThreads'; Value = '321'; Expected = '321'; Default = '1500' },
        @{ Name = 'AppKillProcessTree'; Value = '0'; Expected = '0'; Default = '1' },
        @{ Name = 'AppThrottle'; Value = '321'; Expected = '321'; Default = '1500' },
        @{ Name = 'AppRedirectHook'; Value = '1'; Expected = '1'; Default = '0' },
        @{ Name = 'AppRotateFiles'; Value = '1'; Expected = '1'; Default = '0' },
        @{ Name = 'AppRotateOnline'; Value = '1'; Expected = '1'; Default = '0' },
        @{ Name = 'AppRotateSeconds'; Value = '321'; Expected = '321'; Default = '0' },
        @{ Name = 'AppRotateBytes'; Value = '1234'; Expected = '1234'; Default = '0' },
        @{ Name = 'AppRotateBytesHigh'; Value = '2'; Expected = '2'; Default = '0' },
        @{ Name = 'AppRotateDelay'; Value = '321'; Expected = '321'; Default = '0' },
        @{ Name = 'AppTimestampLog'; Value = '1'; Expected = '1'; Default = '0' }
    )
    foreach ($setting in $dwordSettings) {
        Add-Result $results $Binary "set $($setting.Name)" `
            @('set', $Service, $setting.Name, $setting.Value) | Out-Null
        $configured = Add-Result $results $Binary "get $($setting.Name)" `
            @('get', $Service, $setting.Name)
        Assert-SettingValue $configured $setting.Expected "get $($setting.Name)"
        Add-Result $results $Binary "reset $($setting.Name)" `
            @('reset', $Service, $setting.Name) | Out-Null
        $reset = Add-Result $results $Binary "get default $($setting.Name)" `
            @('get', $Service, $setting.Name)
        Assert-SettingValue $reset $setting.Default "get default $($setting.Name)"
    }
    $temporary = $env:TEMP
    foreach ($setting in @(
        @{ Name = 'AppParameters'; Value = '--label "two words"'; Default = '' },
        @{ Name = 'AppDirectory'; Value = $temporary; Default = '' },
        @{ Name = 'AppStdin'; Value = (Join-Path $temporary "$Service stdin.txt"); Default = '' },
        @{ Name = 'AppStdout'; Value = (Join-Path $temporary "$Service stdout.txt"); Default = '' },
        @{ Name = 'AppStderr'; Value = (Join-Path $temporary "$Service stderr.txt"); Default = '' },
        @{ Name = 'AppAffinity'; Value = '0'; Default = 'All' }
    )) {
        if ($setting.Name -in @('AppParameters', 'AppDirectory')) {
            Compare-StringSetting $results $Binary $Service $setting.Name $setting.Value `
                $setting.Default -SkipReset
        } else {
            Compare-StringSetting $results $Binary $Service $setting.Name $setting.Value $setting.Default
        }
    }
    Compare-StringSetting $results $Binary $Service 'AppExit' 'Ignore' 'Restart' '23'
    Compare-StringSetting $results $Binary $Service 'AppEvents' 'cmd.exe /c exit 0' $null 'Start/Pre'
    foreach ($setting in @('AppEnvironment', 'AppEnvironmentExtra')) {
        Add-Result $results $Binary "set $setting" `
            @('set', $Service, $setting, '+NSSMR_PARITY=value') | Out-Null
        $environmentValue = Add-Result $results $Binary "get $setting key" `
            @('get', $Service, $setting, 'NSSMR_PARITY')
        Assert-SettingValue $environmentValue 'value' "get $setting key"
        Add-Result $results $Binary "reset $setting" `
            @('reset', $Service, $setting) | Out-Null
        Add-Result $results $Binary "get reset $setting key" `
            @('get', $Service, $setting, 'NSSMR_PARITY') | Out-Null
    }
    foreach ($setting in @(
        @{ Name = 'DisplayName'; Value = "$Service reference display name"; Default = $Service },
        @{ Name = 'DependOnService'; Value = 'RpcSs'; Default = $null }
    )) {
        Compare-StringSetting $results $Binary $Service $setting.Name `
            $setting.Value $setting.Default
    }
    Add-Result $results $Binary 'set Start' `
        @('set', $Service, 'Start', 'SERVICE_AUTO_START') | Out-Null
    Add-Result $results $Binary 'get Start' @('get', $Service, 'Start') | Out-Null
    Add-Result $results $Binary 'reset Start without default' `
        @('reset', $Service, 'Start') @(6) | Out-Null
    Add-Result $results $Binary 'get Start after rejected reset' `
        @('get', $Service, 'Start') | Out-Null
    Add-Result $results $Binary 'restore Start' `
        @('set', $Service, 'Start', 'SERVICE_DEMAND_START') | Out-Null
    Add-Result $results $Binary 'set Type' `
        @('set', $Service, 'Type', 'SERVICE_WIN32_OWN_PROCESS') | Out-Null
    Add-Result $results $Binary 'get Type' @('get', $Service, 'Type') | Out-Null
    Add-Result $results $Binary 'reset Type without default' `
        @('reset', $Service, 'Type') @(6) | Out-Null
    Add-Result $results $Binary 'get Type after rejected reset' `
        @('get', $Service, 'Type') | Out-Null
    Add-Result $results $Binary 'set Environment' `
        @('set', $Service, 'Environment', 'NSSMR_NATIVE=value') @(0, 6) | Out-Null
    Add-Result $results $Binary 'get Environment key' `
        @('get', $Service, 'Environment', 'NSSMR_NATIVE') | Out-Null
    Add-Result $results $Binary 'reset Environment' `
        @('reset', $Service, 'Environment') @(0, 6) | Out-Null
    Add-Result $results $Binary 'get reset Environment key' `
        @('get', $Service, 'Environment', 'NSSMR_NATIVE') | Out-Null
    Add-Result $results $Binary 'restore AppParameters' `
        @('set', $Service, 'AppParameters', '-t 127.0.0.1') | Out-Null
    Add-Result $results $Binary 'restore AppDirectory' `
        @('set', $Service, 'AppDirectory', '') | Out-Null
    Add-Result $results $Binary 'start' @('start', $Service) | Out-Null
    Wait-ServiceState $Binary $Service 'SERVICE_RUNNING'
    Add-Result $results $Binary 'status running' @('status', $Service) | Out-Null
    Add-Result $results $Binary 'restart' @('restart', $Service) | Out-Null
    Wait-ServiceState $Binary $Service 'SERVICE_RUNNING'
    Add-Result $results $Binary 'status after restart' @('status', $Service) | Out-Null
    $pause = Add-Result $results $Binary 'pause' @('pause', $Service) @(0, 1)
    if ($pause.ExitCode -eq 0) {
        Wait-ServiceState $Binary $Service 'SERVICE_PAUSED'
    }
    $continue = Add-Result $results $Binary 'continue' @('continue', $Service) @(0, 1)
    if ($continue.ExitCode -eq 0) {
        Wait-ServiceState $Binary $Service 'SERVICE_RUNNING'
    }
    Add-Result $results $Binary 'rotate' @('rotate', $Service) @(0, 1) | Out-Null
    Add-Result $results $Binary 'start already running' @('start', $Service) @(1) | Out-Null
    Add-Result $results $Binary 'stop' @('stop', $Service) | Out-Null
    Wait-ServiceState $Binary $Service 'SERVICE_STOPPED'
    Add-Result $results $Binary 'status stopped' @('status', $Service) | Out-Null
    Add-Result $results $Binary 'stop already stopped' @('stop', $Service) | Out-Null
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
        $referenceStderrHex = [BitConverter]::ToString([Convert]::FromBase64String($reference.Stderr))
        $candidateStderrHex = [BitConverter]::ToString([Convert]::FromBase64String($candidate.Stderr))
        throw "Lifecycle mismatch at $($reference.Label): reference exit=$($reference.ExitCode), candidate exit=$($candidate.ExitCode)`nReference stdout: [$referenceStdout]`nCandidate stdout: [$candidateStdout]`nReference stderr: [$referenceStderr]`nReference stderr hex: [$referenceStderrHex]`nCandidate stderr: [$candidateStderr]`nCandidate stderr hex: [$candidateStderrHex]"
    }
    Write-Host "$($reference.Label): match (exit $($reference.ExitCode))"
}
Write-Host "Matched $($referenceResults.Count) lifecycle operations against the pinned NSSM reference."
