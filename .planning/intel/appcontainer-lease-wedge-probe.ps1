$ErrorActionPreference = 'Continue'
$exe   = 'C:\p22-target\release\wayland-core.exe'
$dir   = "$env:LOCALAPPDATA\Wayland\Core\AppContainerLeases\v1"
$bak   = 'C:\p22-evidence\stale-leases-backup'
$base  = 'C:\p22-appc2'
$stale = 'WCore-storage-00002d20-00000000000000f2.toml'
if (Test-Path $base) { cmd /c "rmdir /s /q $base" 2>&1 | Out-Null }
New-Item -ItemType Directory -Force -Path "$base\home" | Out-Null
$env:HOME = "$base\home"; $env:USERPROFILE = "$base\home"

function New-Repo($path) {
  if (Test-Path $path) { cmd /c "rmdir /s /q $path" 2>&1 | Out-Null }
  New-Item -ItemType Directory -Force -Path $path | Out-Null
  Push-Location $path
  cmd /c "git init -q -b main ." 2>&1 | Out-Null
  cmd /c "git config user.email ci@e.c" 2>&1 | Out-Null
  cmd /c "git config user.name ci" 2>&1 | Out-Null
  Set-Content -Path "$path\README.md" -Value "seed"
  Set-Content -Path "$path\.gitignore" -Value ".swarm-worktrees/"
  cmd /c "git add -A" 2>&1 | Out-Null
  cmd /c "git -c commit.gpgsign=false commit -q -m init" 2>&1 | Out-Null
  Pop-Location
}
function Probe($label) {
  $repo = "$base\repo-$label"
  New-Repo $repo
  $out = & $exe swarm --workers 1 --worker-command "cmd.exe /c exit 0" --repo $repo --base-branch main --timeout 60s 2>&1 | Out-String
  $ok       = ([regex]::Matches($out, '"status": "Succeeded"')).Count
  $disabled = ([regex]::Matches($out, 'sandbox disabled')).Count
  $lease    = ([regex]::Matches($out, 'ACL lease SID/profile mismatch')).Count
  $leases   = (Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue).Count
  Write-Output "APPC label=$label leases=$leases succeeded=$ok disabled=$disabled mismatch=$lease"
  if ($ok -eq 0) {
    $fail = ($out -split "`n" | Select-String 'Failed|error|ERROR' | Select-Object -First 3) -join ' | '
    Write-Output "   FAILTEXT: $fail"
  }
}
foreach ($i in 1,2,3) {
  Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
  Probe "clean-$i"
}
Copy-Item "$bak\$stale" -Destination $dir -Force
Probe 'wedged-1'
Copy-Item "$bak\$stale" -Destination $dir -Force
Probe 'wedged-2'
Get-ChildItem -Path $dir -File -ErrorAction SilentlyContinue | Remove-Item -Force
Probe 'clean-final'
