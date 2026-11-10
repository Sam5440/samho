[CmdletBinding()]
param(
    [ValidateRange(300, 400)][int]$Port = 374,
    [string]$BinaryPath = "",
    # 报告默认拒绝覆盖已有验证证据；显式 -Force 才允许重写。
    [switch]$Force
)
$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $BinaryPath) { $BinaryPath = Join-Path $repoRoot "target\release\rpi.exe" }
$BinaryPath = (Resolve-Path -LiteralPath $BinaryPath).Path
$reportPath = Join-Path $repoRoot "reports\release-web-smoke.json"
if ((Test-Path -LiteralPath $reportPath -PathType Leaf) -and -not $Force) {
    throw "Report already exists: $reportPath. Pass -Force to overwrite existing evidence."
}
$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$smokeRoot = Join-Path $tempParent ("rpi-web-smoke-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $smokeRoot | Out-Null
$utf8 = [Text.UTF8Encoding]::new($false)
$configPath = Join-Path $smokeRoot "runtime.toml"
$workspaceToml = ConvertTo-Json -InputObject $repoRoot -Compress
$dataToml = ConvertTo-Json -InputObject (Join-Path $smokeRoot "data") -Compress
[IO.File]::WriteAllText($configPath, @"
workspace = $workspaceToml
data_dir = $dataToml
http_bind = "127.0.0.1:$Port"
skill_roots = []
"@, $utf8)
$baseUrl = "http://127.0.0.1:$Port"
$service = $null
$stdout = $null
$stderr = $null

function Invoke-SmokeJson([string]$Uri, [string]$Body = "") {
    # Windows PowerShell 5.1 otherwise decodes JSON without a charset as Latin-1.
    if ($Body) {
        $response = Invoke-WebRequest $Uri -UseBasicParsing -Method Post -ContentType "application/json; charset=utf-8" -Body ([Text.Encoding]::UTF8.GetBytes($Body))
    } else {
        $response = Invoke-WebRequest $Uri -UseBasicParsing -TimeoutSec 2
    }
    $parsed = ConvertFrom-Json -InputObject ([Text.Encoding]::UTF8.GetString($response.RawContentStream.ToArray()))
    if ($parsed -is [Array]) {
        foreach ($item in $parsed) { Write-Output $item }
    } else {
        Write-Output $parsed
    }
}

function Start-SmokeService {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $BinaryPath
    $info.Arguments = '--config "' + $configPath + '" serve'
    $info.WorkingDirectory = $repoRoot
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    foreach ($name in @("RPI_DATA_DIR", "RPI_WORKSPACE", "RPI_HTTP_BIND")) { $info.EnvironmentVariables.Remove($name) }
    $script:service = [Diagnostics.Process]::new()
    $script:service.StartInfo = $info
    $null = $script:service.Start()
    $script:stdout = $script:service.StandardOutput.ReadToEndAsync()
    $script:stderr = $script:service.StandardError.ReadToEndAsync()
    for ($i = 0; $i -lt 100; $i++) {
        if ($script:service.HasExited) { throw "Service exited: $($script:stderr.Result)" }
        try {
            $health = Invoke-SmokeJson "$baseUrl/api/health"
            if ($health.status -eq "ok") { return }
        } catch { Start-Sleep -Milliseconds 50 }
    }
    throw "Service did not become healthy"
}
function Stop-SmokeService {
    if ($script:service) {
        if (-not $script:service.HasExited) { $script:service.Kill() }
        $script:service.WaitForExit()
        $script:service.Dispose()
        $script:service = $null
    }
}
function Invoke-SmokeTask($request) {
    $submitted = Invoke-SmokeJson "$baseUrl/api/run" ($request | ConvertTo-Json -Depth 8 -Compress)
    for ($i = 0; $i -lt 100; $i++) {
        $task = Invoke-SmokeJson "$baseUrl/api/tasks/$($submitted.task_id)"
        if ($task.status -ne "running") {
            if ($task.status -ne "completed") { throw "Task failed: $($task | ConvertTo-Json -Depth 8)" }
            return $task
        }
        Start-Sleep -Milliseconds 30
    }
    throw "Task did not finish"
}
try {
    Start-SmokeService
    $html = Invoke-WebRequest $baseUrl -UseBasicParsing
    if ($html.StatusCode -ne 200 -or $html.Content -notmatch '/js/main\.js') { throw "Bundled GUI missing" }
    # 当前入口为 ESM 拆分模块（/js/main.js + /app.css + /ui/*.css）。
    foreach ($asset in @("app.css", "ui/tokens.css", "js/main.js")) {
        $response = Invoke-WebRequest "$baseUrl/$asset" -UseBasicParsing
        if ($response.StatusCode -ne 200) { throw "Bundled asset missing: $asset" }
    }
    $first = Invoke-SmokeTask @{prompt = "/demo read README.md"; profile = @{automatic = $false; complexity = 2}}
    if ($first.result.model_id -ne "demo-fast" -or $first.result.turns -ne 2 -or $first.result.output -notmatch "RPi") { throw "Read tool loop mismatch" }
    $second = Invoke-SmokeTask @{session_id = $first.result.session_id; prompt = "continue the smoke task"; profile = @{automatic = $false; complexity = 7; tier = "reasoning"}}
    if ($second.result.session_id -ne $first.result.session_id -or $second.result.model_id -ne "demo-reasoning") { throw "Session/tier override mismatch" }
    $list = Invoke-SmokeJson "$baseUrl/api/sessions"
    $list = @($list)
    if ($list.Count -ne 1 -or $list[0].PSObject.Properties.Name -contains "messages") { throw "Session summary schema mismatch" }
    $session = Invoke-SmokeJson "$baseUrl/api/sessions/$($first.result.session_id)"
    if ($list[0].message_count -ne $session.messages.Count) { throw "Session message count mismatch" }
    $events = Invoke-SmokeJson "$baseUrl/api/events"
    $events = @($events)
    $state = Invoke-SmokeJson "$baseUrl/api/state"
    if ($state.events -ne $events.Count -or -not $state.simulation) { throw "Cached event count mismatch" }
    if (@($events | Where-Object kind -eq "tool.result").Count -ne 1) { throw "Tool trace missing" }
    $objects = @(Invoke-SmokeJson "$baseUrl/api/objects")
    $skills = @(Invoke-SmokeJson "$baseUrl/api/skills")
    # 对象注册表随内置工具集演进（当前 18 项），断言核心投影而非固定计数。
    $objectIds = @($objects | ForEach-Object { $_.id })
    $modelCount = @($objectIds | Where-Object { $_.StartsWith("model:") }).Count
    $sessionCount = @($objects | Where-Object { $_.id.StartsWith("session:") }).Count
    $hasRuntime = $objectIds.Contains("runtime")
    $hasReadFile = $objectIds.Contains("tool:read_file")
    if ($skills.Count -ne 0 -or $sessionCount -ne 1 -or $modelCount -lt 1 -or -not $hasRuntime -or -not $hasReadFile) { throw "Object/Skill list projection mismatch" }
    $beforeRestart = $events[-1].sequence
    Stop-SmokeService
    Start-SmokeService
    $restored = Invoke-SmokeJson "$baseUrl/api/sessions/$($first.result.session_id)"
    if ($restored.messages.Count -ne $session.messages.Count -or $restored.status -ne "done") { throw "Restart restore mismatch" }
    $third = Invoke-SmokeTask @{session_id = $restored.id; prompt = "after restart"; profile = @{automatic = $false; complexity = 4}}
    $tail = Invoke-SmokeJson "$baseUrl/api/events?after=$beforeRestart"
    $tail = @($tail)
    if ($tail.Count -eq 0 -or $tail[0].sequence -ne ($beforeRestart + 1)) { throw "Restart trace sequence mismatch" }
    $report = [ordered]@{
        schema = "rpi.release-web-smoke.v1"
        generated_at_utc = [DateTime]::UtcNow.ToString("o")
        binary_sha256 = (Get-FileHash -LiteralPath $BinaryPath -Algorithm SHA256).Hash
        passed = $true
        models = @($first.result.model_id, $second.result.model_id, $third.result.model_id)
        object_count = $objects.Count
        skill_count = $skills.Count
        summary_message_count = $list[0].message_count
        events_before_restart = $events.Count
        events_after_restart = $tail.Count
        tasks = @($first, $second, $third)
        limits = "Demo providers only; verifies release assets, HTTP API, real workspace read, shared session, summaries, trace counts and idle restart. Live MCP, real LLM and browser cancellation races are separate tests. Service termination is forced after tasks settle."
    }
    $reportPath = Join-Path $repoRoot "reports\release-web-smoke.json"
    [IO.File]::WriteAllText($reportPath, ($report | ConvertTo-Json -Depth 12), $utf8)
    Write-Output "Release Web smoke passed: $reportPath"
} finally {
    Stop-SmokeService
    $resolved = [IO.Path]::GetFullPath($smokeRoot)
    $expectedParent = $tempParent.TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($expectedParent, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path -Leaf $resolved) -notlike "rpi-web-smoke-*") { throw "Refusing cleanup outside smoke temporary root" }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
