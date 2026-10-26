[CmdletBinding()]
param(
    [string]$BinaryPath = "",
    [string]$OutputPath = "",
    [ValidateRange(0, 1000)][int]$Warmup = 3,
    [ValidateRange(1, 10000)][int]$Samples = 20,
    [ValidateRange(1, 600)][int]$TimeoutSeconds = 30,
    [string]$Prompt = "benchmark a deterministic simulated agent run",
    [switch]$BufferedLogs
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if ([string]::IsNullOrWhiteSpace($BinaryPath)) { $BinaryPath = Join-Path $repoRoot "target\release\rpi.exe" }
if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
    throw "Release binary not found: $BinaryPath. Run cargo build --release -p rpi before benchmarking."
}
$BinaryPath = (Resolve-Path -LiteralPath $BinaryPath).Path
if ([string]::IsNullOrWhiteSpace($OutputPath)) { $OutputPath = Join-Path $repoRoot "reports\benchmark-runtime.json" }
$OutputPath = [IO.Path]::GetFullPath($OutputPath)
New-Item -ItemType Directory -Path (Split-Path -Parent $OutputPath) -Force | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)
$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempRoot = Join-Path $tempParent ("rpi-runtime-benchmark-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempRoot | Out-Null
$durable = -not $BufferedLogs.IsPresent

# Retain the native process handle until peak memory is read, including after exit.
# GetProcessMemoryInfo reports the Windows peak rather than a sampling estimate.
if (-not ("RpiRuntimeBenchmark.Native" -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace RpiRuntimeBenchmark {
    [StructLayout(LayoutKind.Sequential)]
    public struct MemoryCounters {
        public uint cb, PageFaultCount;
        public UIntPtr PeakWorkingSetSize, WorkingSetSize;
        public UIntPtr QuotaPeakPagedPoolUsage, QuotaPagedPoolUsage;
        public UIntPtr QuotaPeakNonPagedPoolUsage, QuotaNonPagedPoolUsage;
        public UIntPtr PagefileUsage, PeakPagefileUsage, PrivateUsage;
    }
    public static class Native {
        [DllImport("psapi.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool GetProcessMemoryInfo(IntPtr process, ref MemoryCounters counters, uint size);
    }
}
'@
}

function ConvertTo-NativeArgument([string]$value) {
    # Windows CRT argv quoting. No shell interprets these arguments.
    $escaped = [regex]::Replace($value, '(\\*)"', '$1$1\"')
    $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
    return '"' + $escaped + '"'
}

function Get-ToolOutput([string]$name, [string[]]$arguments) {
    $tool = Get-Command $name -ErrorAction SilentlyContinue
    if (-not $tool) { return "unavailable" }
    try {
        $result = & $tool.Source @arguments 2>$null
        if ($LASTEXITCODE -ne 0) { return "unavailable" }
        return ($result | Out-String).Trim()
    } catch { return "unavailable" }
}

function Get-Percentile([double[]]$values, [double]$fraction) {
    if ($values.Count -eq 0) { return $null }
    $ordered = @($values | Sort-Object)
    $index = [Math]::Max(0, [Math]::Min($ordered.Count - 1, [Math]::Ceiling($fraction * $ordered.Count) - 1))
    return [double]$ordered[[int]$index]
}

function Invoke-BenchmarkRun([string]$phase, [int]$index) {
    $runRoot = Join-Path $tempRoot ("{0}-{1}" -f $phase, $index)
    $workspace = Join-Path $runRoot "workspace"
    $dataDirectory = Join-Path $runRoot "data"
    $configPath = Join-Path $runRoot "runtime.toml"
    New-Item -ItemType Directory -Path $workspace -Force | Out-Null
    # JSON-encoded strings are also valid TOML basic strings. They safely handle
    # backslashes and quotes in Windows paths without shell interpolation.
    $workspaceToml = ConvertTo-Json -InputObject $workspace -Compress
    $dataToml = ConvertTo-Json -InputObject $dataDirectory -Compress
    $durableToml = $durable.ToString().ToLowerInvariant()
    $configText = @"
workspace = $workspaceToml
data_dir = $dataToml
durable_logs = $durableToml
log_content = false
max_concurrent_tasks = 1
skill_roots = []
mcp_servers = []
[tools]
read = false
write = false
shell = false
mcp = false
[[models]]
id = 'demo-balanced'
model = 'demo'
provider = 'demo'
tier = 'balanced'
"@
    [IO.File]::WriteAllText($configPath, $configText, $utf8)

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $BinaryPath
    $argv = @("--config", $configPath, "run", $Prompt, "--model", "demo-balanced", "--json")
    $startInfo.Arguments = ($argv | ForEach-Object { ConvertTo-NativeArgument $_ }) -join " "
    $startInfo.WorkingDirectory = $workspace
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($name in @($startInfo.EnvironmentVariables.Keys)) {
        if ($name -match '(?i)(api_key|token|secret|password)' -or $name -in @("RPI_WORKSPACE", "RPI_DATA_DIR", "RPI_HTTP_BIND")) {
            $startInfo.EnvironmentVariables.Remove($name)
        }
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $clock = [Diagnostics.Stopwatch]::new()
    try {
        $clock.Start()
        if (-not $process.Start()) { throw "failed to start benchmark process" }
        $processHandle = $process.Handle
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            $process.Kill()
            $process.WaitForExit()
            throw "benchmark process exceeded $TimeoutSeconds seconds"
        }
        $clock.Stop()
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()
        $counters = [RpiRuntimeBenchmark.MemoryCounters]::new()
        $counters.cb = [Runtime.InteropServices.Marshal]::SizeOf($counters)
        $memoryAvailable = [RpiRuntimeBenchmark.Native]::GetProcessMemoryInfo($processHandle, [ref]$counters, $counters.cb)
        $peakBytes = if ($memoryAvailable) { $counters.PeakWorkingSetSize.ToUInt64() } else { $null }
        $parsed = $null
        try { $parsed = $stdout | ConvertFrom-Json } catch { }
        return [pscustomobject][ordered]@{
            phase = $phase
            index = $index
            exit_code = $process.ExitCode
            process_to_completion_ms = [Math]::Round($clock.Elapsed.TotalMilliseconds, 3)
            peak_working_set_bytes = $peakBytes
            memory_measurement = if ($memoryAvailable) { "Windows GetProcessMemoryInfo PeakWorkingSetSize" } else { "unavailable: native counter query failed" }
            status = if ($parsed) { [string]$parsed.status } else { "unparseable" }
            model_id = if ($parsed) { [string]$parsed.model_id } else { "" }
            trace_id = if ($parsed) { [string]$parsed.trace_id } else { "" }
            simulation_label_present = $null -ne $parsed -and ([string]$parsed.output).Contains("[DEMO simulation")
            stdout = $stdout.Trim()
            stderr = $stderr.Trim()
        }
    } finally { $process.Dispose() }
}

$warmupResults = [Collections.Generic.List[object]]::new()
$sampleResults = [Collections.Generic.List[object]]::new()
$failure = $null
try {
    foreach ($phase in @("warmup", "sample")) {
        $count = if ($phase -eq "warmup") { $Warmup } else { $Samples }
        for ($i = 1; $i -le $count; $i++) {
            $result = Invoke-BenchmarkRun $phase $i
            if ($phase -eq "warmup") { $warmupResults.Add($result) } else { $sampleResults.Add($result) }
            if ($result.exit_code -ne 0 -or $result.status -ne "done" -or $result.model_id -ne "demo-balanced" -or -not $result.simulation_label_present) {
                throw ("{0} {1} failed demo validation: exit={2}, status={3}, model={4}" -f $phase, $i, $result.exit_code, $result.status, $result.model_id)
            }
            Write-Host ("{0} {1}/{2}: {3} ms, {4} peak bytes" -f $phase, $i, $count, $result.process_to_completion_ms, $result.peak_working_set_bytes)
        }
    }
} catch { $failure = $_.Exception.Message }
finally {
    $times = @($sampleResults | Where-Object { $_.exit_code -eq 0 -and $_.status -eq "done" } | ForEach-Object { [double]$_.process_to_completion_ms })
    $memory = @($sampleResults | Where-Object { $null -ne $_.peak_working_set_bytes } | ForEach-Object { [double]$_.peak_working_set_bytes })
    $cpu = try { Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors } catch { "unavailable" }
    $computer = try { Get-CimInstance Win32_ComputerSystem | Select-Object Manufacturer, Model, TotalPhysicalMemory } catch { "unavailable" }
    $os = try { Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, OSArchitecture } catch { [Environment]::OSVersion.VersionString }
    $report = [ordered]@{
        schema = "rpi.runtime-benchmark.v1"
        generated_at_utc = [DateTime]::UtcNow.ToString("o")
        completed = $null -eq $failure
        failure = $failure
        binary = $BinaryPath
        binary_sha256 = (Get-FileHash -LiteralPath $BinaryPath -Algorithm SHA256).Hash
        binary_bytes = (Get-Item -LiteralPath $BinaryPath).Length
        workspace_commit = Get-ToolOutput "git" @("-C", $repoRoot, "rev-parse", "HEAD")
        upstream_pi_commit = Get-ToolOutput "git" @("-C", (Join-Path $repoRoot "upstream\pi"), "rev-parse", "HEAD")
        rustc = Get-ToolOutput "rustc" @("--version")
        node = Get-ToolOutput "node" @("--version")
        powershell = $PSVersionTable.PSVersion.ToString()
        hardware = [ordered]@{ os = $os; cpu = $cpu; computer = $computer }
        provider = "demo"
        network = "not used: one built-in Demo model; file/shell/MCP tools disabled, no Skill roots; built-in load_skill schema remains advertised; task invokes no tools"
        durable_logs = $durable
        prompt = $Prompt
        warmup_requested = $Warmup
        samples_requested = $Samples
        warmup_completed = $warmupResults.Count
        samples_completed = $sampleResults.Count
        metric_definition = "External fresh-process launch through process exit: process startup + runtime initialization + routing + simulated task + journal writes + shutdown. Separate startup/runtime phases are not instrumented. OS caches may be warm after preheating. Not a Pi/Node speedup or real Agent quality claim."
        percentile_method = "nearest rank; sorted ascending, ceil(p*n)-1"
        summary = [ordered]@{
            process_to_completion_ms = [ordered]@{ min = ($times | Measure-Object -Minimum).Minimum; p50 = Get-Percentile $times 0.5; p95 = Get-Percentile $times 0.95; max = ($times | Measure-Object -Maximum).Maximum }
            peak_working_set_bytes = [ordered]@{ min = ($memory | Measure-Object -Minimum).Minimum; p50 = Get-Percentile $memory 0.5; p95 = Get-Percentile $memory 0.95; max = ($memory | Measure-Object -Maximum).Maximum; available_samples = $memory.Count }
        }
        warmups = @($warmupResults.ToArray())
        samples = @($sampleResults.ToArray())
    }
    [IO.File]::WriteAllText($OutputPath, ($report | ConvertTo-Json -Depth 10), $utf8)
    # Verify this uniquely created temp directory before the only recursive delete.
    $cleanupTarget = (Resolve-Path -LiteralPath $tempRoot).Path
    $expectedParent = $tempParent.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    if (-not $cleanupTarget.StartsWith($expectedParent, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path -Leaf $cleanupTarget) -notmatch '^rpi-runtime-benchmark-[a-f0-9]{32}$') {
        throw "Refusing cleanup of unexpected benchmark directory: $cleanupTarget"
    }
    Remove-Item -LiteralPath $cleanupTarget -Recurse -Force
    Write-Host "Wrote $OutputPath"
}
if ($failure) { throw $failure }
