# 28-E5-REPAIR — launch the matrix leg the way 28-02 launched it: as a SCHEDULED TASK,
# not straight off the ssh session. 28-02's own observability control measured both session
# types and found the session type made no difference, but the row this run is being compared
# against (219 pass / 0 red) was produced by a scheduled task, so the scheduled task is what
# gets reproduced.
#
# -WorkingDirectory is set EXPLICITLY: scheduled tasks inherit the process working directory,
# not a PowerShell provider location.
param(
  [Parameter(Mandatory=$true)][string]$Commit,
  [Parameter(Mandatory=$true)][string]$Tree,
  [Parameter(Mandatory=$true)][string]$Nonce,
  [Parameter(Mandatory=$true)][string]$ExpectedSha256
)
$ErrorActionPreference = 'Stop'
$name = 'f28e5WinMatrix'
Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue

$argline = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\f28e5\f28e5-win-matrix.ps1 " +
           "-Commit $Commit -Tree $Tree -Nonce $Nonce -ExpectedSha256 $ExpectedSha256 -AllowLoad"

$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $argline -WorkingDirectory 'C:\f28e5'
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Highest
$settings  = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
             -ExecutionTimeLimit (New-TimeSpan -Hours 4) -MultipleInstances IgnoreNew

Register-ScheduledTask -TaskName $name -Action $action -Principal $principal -Settings $settings | Out-Null
Start-ScheduledTask -TaskName $name
Start-Sleep -Seconds 5
$t = Get-ScheduledTask -TaskName $name
Write-Output ("TASK_STATE=" + $t.State)
Write-Output "REGISTERED_OK"
exit 0
