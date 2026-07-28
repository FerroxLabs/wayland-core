# 28-E5-REPAIR — launch the wedge differential as a scheduled task, same session type as the
# main matrix leg, so the two are directly comparable.
$ErrorActionPreference = 'Stop'
$name = 'f28e5WedgeDiff'
Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
$argline = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\f28e5\f28e5-wedge-differential.ps1 ' +
  '-PreExe C:\f28e5\pre-repair-wayland-core.exe ' +
  '-PreSha baf9bd692833eb7b9d54f00053739115b6ad5257fbdb0b0e99a8694a2ee996a6 ' +
  '-PreCommit 32e2f57d09fe4b287e513081862217dc9daa5901 ' +
  '-PreTree 63ec0e6c36ff8e63789aab2f9760870304b671df ' +
  '-PostExe C:\f28e5\candidate-wayland-core.exe ' +
  '-PostSha 4c48d6656f1d640fe1dbff7f2cceaaa260bca3b12ce51b930d7b9541d6d41f9d ' +
  '-PostCommit 6db9e56b8b6c68a2b7939a0728beb06a92ceed0b ' +
  '-PostTree 1533c6a42b26522ee553e90954dd159bbaed2c3b ' +
  '-NonceA c8a7d356adc3c81d48805b4b6d081c08 -NonceB 50e0808f020eca09dfe9c6f2058d7f04'
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $argline -WorkingDirectory 'C:\f28e5'
$principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Highest
$settings  = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 4) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $name -Action $action -Principal $principal -Settings $settings | Out-Null
Start-ScheduledTask -TaskName $name
Start-Sleep -Seconds 5
Write-Output ('TASK_STATE=' + (Get-ScheduledTask -TaskName $name).State)
Write-Output 'REGISTERED_OK'
exit 0
