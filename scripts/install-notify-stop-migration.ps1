# Does not run at app startup. Parent/operator reviews and invokes explicitly.
[CmdletBinding(SupportsShouldProcess)]
param([Parameter(Mandatory)][string]$HookPath, [string]$SuppressedResumeFile)
$ErrorActionPreference = 'Stop'
$hook = (Resolve-Path -LiteralPath $HookPath).Path
if (-not (Test-Path -LiteralPath $hook -PathType Leaf) -or [IO.Path]::GetExtension($hook) -ine '.ps1') { throw 'HookPath must name one existing .ps1 file.' }
$configPath = $hook + '.pullcept.json'
if (Test-Path -LiteralPath $configPath) { throw 'Already migrated; preserve the existing backup and review it before another migration.' }
$suppressed = @()
if ($SuppressedResumeFile) {
    $suppressed = @(Get-Content -LiteralPath $SuppressedResumeFile -Raw -Encoding utf8 | ConvertFrom-Json)
    foreach ($entry in $suppressed) {
        if (-not $entry.task -or -not $entry.project -or [string]$entry.server -notlike 'pullcept-room-*') { throw 'Each suppressed resume needs an exact task, project and registration server.' }
        $entry.project = [IO.Path]::GetFullPath([string]$entry.project)
    }
}
if ($PSCmdlet.ShouldProcess($hook, 'Back up and install the Pullcept notice migration wrapper')) {
    $backup = $hook + '.legacy-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N') + '.ps1'
    Copy-Item -LiteralPath $hook -Destination $backup
    if ((Get-FileHash -LiteralPath $hook).Hash -cne (Get-FileHash -LiteralPath $backup).Hash) { throw 'Backup verification failed; hook not changed.' }
    # Keep the byte-identical backup; only the execution copy redirects future
    # fallback reservations through the wrapper and its scoped Resume ledger.
    $legacy = [IO.File]::ReadAllText($backup)
    $scheduler = '-f $PSCommandPath, $server, $dir, $name'
    if ($legacy.Contains('Register-ResumeTask') -and -not $legacy.Contains($scheduler)) { throw 'Unknown legacy scheduler; backup kept and original hook unchanged. Review before migration.' }
    $quotedHook = "'" + $hook.Replace("'", "''") + "'"
    $runner = $backup + '.fallback.ps1'
    [IO.File]::WriteAllText($runner, $legacy.Replace($scheduler, ('-f ' + $quotedHook + ', $server, $dir, $name')), [Text.UTF8Encoding]::new($false))
    $config = @{ backup_path = $backup; legacy_path = $runner; suppressed = $suppressed }
    $utf8 = [Text.UTF8Encoding]::new($false)
    [IO.File]::WriteAllText($configPath, ($config | ConvertTo-Json -Depth 8), $utf8)
    $template = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'notify-stop-migration.ps1'))
    $next = $hook + '.next-' + [guid]::NewGuid().ToString('N')
    [IO.File]::WriteAllText($next, $template, $utf8)
    Move-Item -LiteralPath $next -Destination $hook -Force
    Write-Output "Backup: $backup"
}
