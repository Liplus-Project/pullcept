# Wrapper template; installed only by an explicitly invoked migration script.
param([string]$Resume, [string]$ProjectDir, [string]$TaskName)
$ErrorActionPreference = 'Stop'
try {
    $config = Get-Content -LiteralPath ($PSCommandPath + '.pullcept.json') -Raw -Encoding utf8 | ConvertFrom-Json
    if ($Resume) {
        # Exact task + registration + project scope. Never a global task-name glob.
        $suppressed = @($config.suppressed | Where-Object {
            [string]$_.task -ceq $TaskName -and [string]$_.server -ceq $Resume -and
            [IO.Path]::GetFullPath([string]$_.project) -ieq [IO.Path]::GetFullPath($ProjectDir)
        })
        if ($suppressed.Count -gt 0) { exit 0 }
        & (Get-Process -Id $PID).Path -NoProfile -NonInteractive -WindowStyle Hidden -File $config.legacy_path -Resume $Resume -ProjectDir $ProjectDir -TaskName $TaskName *> $null
        exit 0
    }
    $raw = [Console]::In.ReadToEnd()
    $body = $raw | ConvertFrom-Json
    # No permission-prompt notification is introduced by this wrapper.
    if ($body.hook_event_name -eq 'Notification' -and $body.notification_type -eq 'permission_prompt') { exit 0 }
    if ($body.hook_event_name -eq 'StopFailure' -and $body.error -eq 'rate_limit' -and
        $env:PULLCEPT_LIMIT_NOTICE_OWNER -ceq 'app-v1' -and $env:PULLCEPT_CLAUDE_LAUNCH) { exit 0 }
    # Existing launches and non-limit API errors retain the backed-up handler.
    $raw | & (Get-Process -Id $PID).Path -NoProfile -NonInteractive -WindowStyle Hidden -File $config.legacy_path *> $null
} catch { }
exit 0
