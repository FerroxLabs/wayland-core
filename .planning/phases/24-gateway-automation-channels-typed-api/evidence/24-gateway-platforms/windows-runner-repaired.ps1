$ErrorActionPreference = 'Continue'
$root   = 'D:\lane-gateway-platforms'
$status = "$root\run\status.txt"
$log    = "$root\run\journey.log"
$rundir = "$root\run\windows"
$samp   = "$root\run\proc-sample.txt"

Remove-Item -Force   -ErrorAction SilentlyContinue $status
Remove-Item -Force   -ErrorAction SilentlyContinue $log
Remove-Item -Force   -ErrorAction SilentlyContinue $samp
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $rundir
New-Item -ItemType Directory -Force -Path $rundir | Out-Null

# Sampler: how many wayland-core runtimes exist, every 5s. This is the
# discriminator between "two concurrent runtimes" (single-instance lock failed)
# and "one runtime restarted and re-fired its cron jobs".
$sampler = Start-Job -ScriptBlock {
  param($out)
  for ($i = 0; $i -lt 200; $i++) {
    $p = @(Get-Process wayland-core -ErrorAction SilentlyContinue)
    $pids = ($p | ForEach-Object { $_.Id }) -join ','
    Add-Content -Path $out -Value ("{0} count={1} pids={2}" -f (Get-Date -Format 'HH:mm:ss'), $p.Count, $pids)
    Start-Sleep -Seconds 5
  }
} -ArgumentList $samp

Set-Location 'D:\lane-gwplat-repo'
& node "D:\lane-gwplat-repo\scripts\f24-journey.mjs" --platform windows --run-dir $rundir --binary "$root\wayland-core.exe" *> $log
$rc = $LASTEXITCODE

Stop-Job $sampler -ErrorAction SilentlyContinue
Remove-Job $sampler -Force -ErrorAction SilentlyContinue

Set-Content -Path $status -Value "WLRC=${rc}"
Add-Content -Path $status -Value "WLDONE"
