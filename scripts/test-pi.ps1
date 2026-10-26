param(
    [string]$WorkspaceRoot = "D:\ddcode\rpi",
    [string]$Package = "",
    [string]$Command = "npm test",
    [string]$ReportName = "pi-tests-windows"
)
$ErrorActionPreference = "Stop"
$piRoot = Join-Path $WorkspaceRoot "upstream\pi"
$reportRoot = Join-Path $WorkspaceRoot "reports"
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("rpi-pi-test-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $testRoot, $reportRoot -Force | Out-Null
$startInfo = New-Object System.Diagnostics.ProcessStartInfo
$startInfo.FileName = Join-Path $env:SystemRoot "System32\cmd.exe"
$startInfo.Arguments = "/d /c " + $Command
$startInfo.WorkingDirectory = if ($Package) { Join-Path $piRoot $Package } else { $piRoot }
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$startInfo.EnvironmentVariables.Clear()
foreach ($name in @("PATH", "SystemRoot", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT")) {
    $value = [Environment]::GetEnvironmentVariable($name)
    if ($value) { $startInfo.EnvironmentVariables[$name] = $value }
}
$isolated = @{
    HOME = $testRoot
    USERPROFILE = $testRoot
    TEMP = $testRoot
    TMP = $testRoot
    XDG_CONFIG_HOME = (Join-Path $testRoot ".config")
    XDG_CACHE_HOME = (Join-Path $testRoot "cache")
    GIT_CONFIG_NOSYSTEM = "1"
    GIT_TERMINAL_PROMPT = "0"
    GIT_CONFIG_GLOBAL = (Join-Path $testRoot "gitconfig")
    GIT_EDITOR = "true"
    GIT_SEQUENCE_EDITOR = "true"
    NPM_CONFIG_USERCONFIG = (Join-Path $testRoot "npmrc")
    PI_NO_LOCAL_LLM = "1"
    AWS_EC2_METADATA_DISABLED = "true"
    TZ = "UTC"
    LANG = "C"
    CI = "1"
}
foreach ($entry in $isolated.GetEnumerator()) { $startInfo.EnvironmentVariables[$entry.Key] = $entry.Value }
New-Item -ItemType File -Path $isolated.GIT_CONFIG_GLOBAL, $isolated.NPM_CONFIG_USERCONFIG -Force | Out-Null
$process = New-Object System.Diagnostics.Process
$process.StartInfo = $startInfo
$null = $process.Start()
$stdout = $process.StandardOutput.ReadToEndAsync()
$stderr = $process.StandardError.ReadToEndAsync()
$process.WaitForExit()
$log = $stdout.Result + [Environment]::NewLine + $stderr.Result
[System.IO.File]::WriteAllText((Join-Path $reportRoot ($ReportName + ".log")), $log, [System.Text.Encoding]::UTF8)
$summary = @{ exit_code = $process.ExitCode; command = $Command; package = $Package; environment = "isolated Windows process without provider credentials"; test_home = $testRoot }
[System.IO.File]::WriteAllText((Join-Path $reportRoot ($ReportName + ".json")), ($summary | ConvertTo-Json), [System.Text.Encoding]::UTF8)
Write-Output ("Pi full-suite exit: " + $process.ExitCode)
Write-Output ("Report: " + (Join-Path $reportRoot ($ReportName + ".log")))
exit $process.ExitCode
