Remove-Item -Force -ErrorAction SilentlyContinue C:\Users\seand\f28e5-cleanup.ps1
Remove-Item -Force -ErrorAction SilentlyContinue C:\Users\seand\f28e5-cleanup.log
Write-Output ("RESIDUE_HOME=" + ((Get-ChildItem C:\Users\seand -Filter 'f28e5*' -ErrorAction SilentlyContinue | Measure-Object).Count))
Write-Output ("F28E5_DIR=" + (Test-Path C:\f28e5))
Write-Output ("F28_TASKS=" + ((Get-ScheduledTask -TaskName 'f28e5*' -ErrorAction SilentlyContinue | Measure-Object).Count))
