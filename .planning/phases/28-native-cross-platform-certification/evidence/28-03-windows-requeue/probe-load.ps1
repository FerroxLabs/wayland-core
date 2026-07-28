$ErrorActionPreference = 'Continue'
Write-Output "NOW=$(Get-Date -Format o)"
Write-Output "CPUS=$($env:NUMBER_OF_PROCESSORS)"
foreach ($p in Get-Process -Name cargo,rustc,node,wayland-core -ErrorAction SilentlyContinue) {
  Write-Output ("PROC pid={0} name={1} start={2} cpu={3} path={4}" -f $p.Id, $p.ProcessName, $p.StartTime, $p.CPU, $p.Path)
}
# Which worktrees have a recently-written target dir -- identifies whose build this is.
foreach ($d in Get-ChildItem C:\ -Directory -ErrorAction SilentlyContinue) {
  $t = Join-Path $d.FullName 'target'
  if (Test-Path $t) {
    $last = (Get-ChildItem $t -Recurse -File -ErrorAction SilentlyContinue |
             Sort-Object LastWriteTime -Descending | Select-Object -First 1)
    if ($last -and $last.LastWriteTime -gt (Get-Date).AddMinutes(-30)) {
      Write-Output ("ACTIVE_TARGET dir={0} last_write={1}" -f $d.FullName, $last.LastWriteTime)
    }
  }
}
Write-Output "PROBE_DONE"
exit 0
