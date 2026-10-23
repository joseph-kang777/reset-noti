# 리셋권 알림을 작업 스케줄러에 등록 (로그온 시 + 1시간마다, 창 없이 실행)
$ErrorActionPreference = 'Stop'
$script = Join-Path $PSScriptRoot 'reset_noti.py'
$python = (& python -c "import sys; print(sys.executable)").Trim()
$pythonw = Join-Path (Split-Path $python) 'pythonw.exe'
if (-not (Test-Path $pythonw)) { throw "pythonw.exe not found next to $python" }

$action = New-ScheduledTaskAction -Execute $pythonw -Argument "`"$script`"" -WorkingDirectory $PSScriptRoot
$hourly = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Hours 1)
$logon = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit (New-TimeSpan -Minutes 5) -MultipleInstances IgnoreNew

Register-ScheduledTask -TaskName 'ResetNoti' -Action $action -Trigger $hourly, $logon -Settings $settings `
    -Description 'Claude / Codex 리셋권 알림' -Force | Out-Null
Write-Host "등록 완료: ResetNoti ($pythonw)"
Start-ScheduledTask -TaskName 'ResetNoti'
Write-Host "지금 한 번 실행했어요. 기록은 reset_noti.log 에서 볼 수 있어요."
