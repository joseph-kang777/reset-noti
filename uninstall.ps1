# 작업 스케줄러에서 리셋권 알림 제거
Unregister-ScheduledTask -TaskName 'ResetNoti' -Confirm:$false
Remove-Item 'HKCU:\Software\Classes\AppUserModelId\ResetNoti' -Recurse -ErrorAction SilentlyContinue
Write-Host '제거 완료'
