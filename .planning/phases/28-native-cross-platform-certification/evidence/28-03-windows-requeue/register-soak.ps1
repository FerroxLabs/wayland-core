$ErrorActionPreference = 'Stop'
$name = 'wlWinRequeueSoak'
Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue

# -WorkingDirectory is set EXPLICITLY. Start-Process and scheduled tasks inherit the process
# working directory, not a PowerShell provider location, and Set-Location does not affect it.
# A sibling lane lost ninety minutes to a build that silently ran in the ssh home directory.
$action = New-ScheduledTaskAction `
  -Execute 'powershell.exe' `
  -Argument '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\wl-winrequeue\soak-run.ps1' `
  -WorkingDirectory 'C:\wl-winrequeue'

$principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
$settings  = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
             -ExecutionTimeLimit (New-TimeSpan -Hours 4) -MultipleInstances IgnoreNew

Register-ScheduledTask -TaskName $name -Action $action -Principal $principal -Settings $settings | Out-Null
Start-ScheduledTask -TaskName $name
Start-Sleep -Seconds 3
$t = Get-ScheduledTask -TaskName $name
Write-Output "TASK_STATE=$($t.State)"
Write-Output "REGISTERED_OK"
exit 0
