# Lane 28-adj2 — mutation battery. Each mutant restores one defect; the test
# named for it MUST go red, and the others MUST stay green.
param([string]$Mutant = 'm3')
$ErrorActionPreference = 'Continue'
$log    = "C:\f28h2\adj2-mut-$Mutant.log"
$status = "C:\f28h2\adj2-mut-$Mutant.status"
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28ADJ2 MUT $Mutant started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }
$env:CARGO_TARGET_DIR = 'C:\f28h2-target'
$target = 'C:\f28h2-repo\crates\wcore-sandbox\src\backends\appcontainer\acl_lease.rs'

Copy-Item "C:\f28h2\adj2.$Mutant.rs" -Destination $target -Force
# Copy-Item PRESERVES LastWriteTime; without this stamp cargo reuses the stale
# binary and the previous mutant's result is reported as this one's.
(Get-Item $target).LastWriteTime = Get-Date
L ("APPLIED=" + $Mutant)
L ("TARGET_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower())

$names = @(
  'dead_owner_unreconcilable_lease_is_reclaimed_not_refused_forever',
  'live_owner_unreconcilable_lease_is_honoured_not_reclaimed',
  'quarantine_directory_does_not_become_a_second_wedge',
  'reclamation_reports_grants_it_could_not_revoke',
  'zero_length_lease_is_reclaimed_not_refused_forever',
  'a_non_empty_unreadable_lease_still_fails_closed',
  'zero_length_lease_is_reachable_through_the_writer',
  'a_leaked_test_lease_is_diagnosed_by_name'
)

Push-Location 'C:\f28h2-repo'
$b = (& cargo build -p wcore-sandbox --tests 2>&1 | Out-String)
$built = ($b -match 'Compiling wcore-sandbox')
L ("MUT_COMPILED=" + $built)
if (-not $built) { L ($b) }

$listing = (& cargo test -p wcore-sandbox --lib -- --list 2>&1 | Out-String)
$resolved = @{}
foreach ($n in $names) {
  $m = [regex]::Match($listing, '(?m)^(\S*' + [regex]::Escape($n) + ')\s*:\s*test')
  if ($m.Success) { $resolved[$n] = $m.Groups[1].Value }
}
L ("RESOLVED_COUNT=" + $resolved.Count + ";EXPECTED=" + $names.Count)

foreach ($n in $names) {
  if (-not $resolved.ContainsKey($n)) { L ("MUT=" + $n + ";result=UNRESOLVED"); continue }
  $o = (& cargo test -p wcore-sandbox --lib -- --exact $resolved[$n] --test-threads=1 2>&1 | Out-String)
  $ms = [regex]::Matches($o, 'test result: (\w+)\. (\d+) passed; (\d+) failed')
  if ($ms.Count -gt 0) {
    $s = $ms[$ms.Count - 1]
    L ("MUT=" + $n + ";result=" + $s.Groups[1].Value + ";passed=" + $s.Groups[2].Value + ";failed=" + $s.Groups[3].Value)
  } else { L ("MUT=" + $n + ";result=UNPARSEABLE") }
}

$full = (& cargo test -p wcore-sandbox --lib -- --test-threads=1 2>&1 | Out-String)
$ms = [regex]::Matches($full, 'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored')
if ($ms.Count -gt 0) {
  $s = $ms[$ms.Count - 1]
  L ("MUT_FULL=result=" + $s.Groups[1].Value + ";passed=" + $s.Groups[2].Value + ";failed=" + $s.Groups[3].Value + ";ignored=" + $s.Groups[4].Value)
} else { L 'MUT_FULL=UNPARSEABLE' }
Pop-Location

Copy-Item 'C:\f28h2\adj2-pristine.rs' -Destination $target -Force
(Get-Item $target).LastWriteTime = Get-Date
L ("RESTORED_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower())
Push-Location 'C:\f28h2-repo'
L ("RESTORED_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)
Pop-Location
L ("finished=" + (Get-Date -Format o))
$rc = 0
if (-not $built) { $rc = 6 }
Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
