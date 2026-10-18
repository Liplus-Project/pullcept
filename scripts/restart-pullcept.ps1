# Rebuild and restart the dev Pullcept of the clone this script sits in (#408).
# Stops the running `npm run tauri dev` tree whose pullcept.exe was built in this
# clone's target directory, starts `npm.cmd run tauri dev` in the clone root, and
# waits until a new pullcept.exe from that target directory is running.
# Every step is appended to restart-pullcept.log (clone root by default).
#
# Without -InPlace the script relaunches itself through WMI (Win32_Process.Create),
# whose parent is outside the Pullcept tree, and returns at once. That is what lets
# a seat terminal inside Pullcept run it: the seat's shell dies with the old tree,
# the relaunched copy does not.
[CmdletBinding()]
param(
    [string]$RepoRoot = (Split-Path -Parent $PSScriptRoot),
    [string]$LogPath,
    [int]$TimeoutSeconds = 900,
    [switch]$InPlace,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'
$RepoRoot = [IO.Path]::GetFullPath($RepoRoot)
if (-not $LogPath) { $LogPath = Join-Path $RepoRoot 'restart-pullcept.log' }
$LogPath = [IO.Path]::GetFullPath($LogPath)
$devLog = [IO.Path]::ChangeExtension($LogPath, '.dev.log')

function Write-Log([string]$Message) {
    $line = '{0} {1}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Message
    Add-Content -LiteralPath $LogPath -Value $line -Encoding utf8
    Write-Output $line
}

if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot 'src-tauri\tauri.conf.json') -PathType Leaf)) {
    throw "Not a Pullcept clone: $RepoRoot"
}

if (-not $InPlace -and -not $DryRun) {
    $hostExe = (Get-Process -Id $PID).Path
    $quote = { param($s) '"' + $s.Replace('"', '""') + '"' }
    $cmd = '{0} -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File {1} -InPlace -RepoRoot {2} -LogPath {3} -TimeoutSeconds {4}' -f `
        (& $quote $hostExe), (& $quote $PSCommandPath), (& $quote $RepoRoot), (& $quote $LogPath), $TimeoutSeconds
    $result = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $cmd; CurrentDirectory = $RepoRoot }
    if ($result.ReturnValue -ne 0) { throw "Win32_Process.Create failed: $($result.ReturnValue)" }
    Write-Output "Restart started detached (PID $($result.ProcessId)). Log: $LogPath"
    return
}

# The target directory: src-tauri\target, or target-dir from the machine-local
# src-tauri\.cargo\config.toml (README: MinGW and paths with spaces).
$tauriDir = Join-Path $RepoRoot 'src-tauri'
$targetDir = Join-Path $tauriDir 'target'
$cargoConfig = Join-Path $tauriDir '.cargo\config.toml'
if (Test-Path -LiteralPath $cargoConfig) {
    $m = Select-String -LiteralPath $cargoConfig -Pattern '^\s*target-dir\s*=\s*["''](.+?)["'']' | Select-Object -First 1
    if ($m) {
        $value = $m.Matches[0].Groups[1].Value
        $targetDir = if ([IO.Path]::IsPathRooted($value)) { $value } else { Join-Path $tauriDir $value }
    }
}
$targetPrefix = [IO.Path]::GetFullPath($targetDir).TrimEnd('\') + '\'

function Get-ProcessTable { @(Get-CimInstance -ClassName Win32_Process) }
function Get-OwnPullcept($table) {
    @($table | Where-Object { $_.Name -ieq 'pullcept.exe' -and $_.ExecutablePath -and $_.ExecutablePath.StartsWith($targetPrefix, [StringComparison]::OrdinalIgnoreCase) })
}

Write-Log "---- restart: repo=$RepoRoot target=$targetPrefix dryrun=$([bool]$DryRun)"
$table = Get-ProcessTable
$byPid = @{}; foreach ($p in $table) { $byPid[[int]$p.ProcessId] = $p }

# Own ancestry: never kill the process running this script.
$self = @{}
$cursor = $byPid[[int]$PID]
while ($cursor -and -not $self.ContainsKey([int]$cursor.ProcessId)) {
    $self[[int]$cursor.ProcessId] = $true
    $cursor = $byPid[[int]$cursor.ParentProcessId]
}

# Climb from each pullcept.exe to the topmost `tauri dev` launcher (npm.cmd's cmd,
# npm's node, the tauri CLI's node / cargo) so the dev server goes down with it.
$roots = @{}
foreach ($app in (Get-OwnPullcept $table)) {
    $top = $app
    while ($true) {
        $parent = $byPid[[int]$top.ParentProcessId]
        if (-not $parent -or $self.ContainsKey([int]$parent.ProcessId)) { break }
        if ($parent.CreationDate -gt $top.CreationDate) { break }  # reused PID
        if ($parent.Name -notin @('node.exe', 'cmd.exe', 'cargo.exe')) { break }
        if ([string]$parent.CommandLine -notmatch 'tauri') { break }
        $top = $parent
    }
    if ($self.ContainsKey([int]$top.ProcessId)) {
        Write-Log "ABORT: pullcept.exe $($app.ProcessId) is an ancestor of this script; run without -InPlace."
        exit 2
    }
    $roots[[int]$top.ProcessId] = $top
    Write-Log "found pullcept.exe pid=$($app.ProcessId) tree-root pid=$($top.ProcessId) ($($top.Name))"
}
if ($roots.Count -eq 0) { Write-Log 'no running Pullcept from this clone' }
$oldPids = @((Get-OwnPullcept $table) | ForEach-Object { [int]$_.ProcessId })

if ($DryRun) { Write-Log 'dry run: nothing stopped or started'; exit 0 }

foreach ($root in $roots.Values) {
    Write-Log "taskkill /T /F /PID $($root.ProcessId)"
    $out = & taskkill.exe /T /F /PID $root.ProcessId 2>&1
    foreach ($l in $out) { Write-Log "  $l" }
}
$deadline = (Get-Date).AddSeconds(30)
while ((Get-OwnPullcept (Get-ProcessTable)).Count -gt 0) {
    if ((Get-Date) -gt $deadline) { Write-Log 'FAIL: old pullcept.exe still running after 30s'; exit 1 }
    Start-Sleep -Milliseconds 500
}
Write-Log 'old tree stopped'

$startedAt = Get-Date
Write-Log "start: npm.cmd run tauri dev (output: $devLog)"
$dev = Start-Process -FilePath 'cmd.exe' -WorkingDirectory $RepoRoot -WindowStyle Hidden -PassThru `
    -ArgumentList @('/d', '/c', ('npm.cmd run tauri dev > "{0}" 2>&1' -f $devLog))
Write-Log "launcher pid=$($dev.Id)"

$deadline = $startedAt.AddSeconds($TimeoutSeconds)
while ((Get-Date) -lt $deadline) {
    $new = @(Get-OwnPullcept (Get-ProcessTable) | Where-Object { [int]$_.ProcessId -notin $oldPids -and $_.CreationDate -ge $startedAt })
    if ($new.Count -gt 0) {
        Write-Log "OK: new pullcept.exe pid=$($new[0].ProcessId) after $([int]((Get-Date) - $startedAt).TotalSeconds)s"
        exit 0
    }
    if ($dev.HasExited) {
        Write-Log "FAIL: tauri dev exited with code $($dev.ExitCode) before pullcept.exe started"
        if (Test-Path -LiteralPath $devLog) { Get-Content -LiteralPath $devLog -Tail 30 | ForEach-Object { Write-Log "  $_" } }
        exit 1
    }
    Start-Sleep -Seconds 2
}
Write-Log "FAIL: no new pullcept.exe within $TimeoutSeconds s (tauri dev left running, see $devLog)"
exit 1
