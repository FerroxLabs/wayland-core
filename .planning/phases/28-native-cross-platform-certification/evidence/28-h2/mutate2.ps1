# Lane 28-h2 — mutation check, second attempt.
#
# The first attempt was WRONG and reported a stale result: Copy-Item preserves
# the source file's LastWriteTime, all three variants were scp'd inside the same
# second, so cargo saw no mtime change and silently re-ran the PREVIOUS mutant's
# binary. M2 therefore reproduced M1's failure exactly. That is the same defect
# class this whole lane is about -- a check that cannot observe what it claims.
#
# Two guards, because the first one alone is unverifiable:
#   1. stamp LastWriteTime to now, so cargo must reconsider the file;
#   2. ASSERT that `Compiling wcore-sandbox` actually appeared. If the mutant was
#      not compiled, the run is graded NOTBUILT and no result is recorded.
param([string]$Mutant = 'm1')
$ErrorActionPreference = 'Continue'
$target = 'C:\f28h2-repo\crates\wcore-sandbox\src\backends\appcontainer\acl_lease.rs'
$log    = "C:\f28h2\mutate2-$Mutant.log"
$status = "C:\f28h2\mutate2-$Mutant.status"
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28H2 MUTATION2 $Mutant started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }

$env:CARGO_TARGET_DIR = 'C:\f28h2-target'
Copy-Item "C:\f28h2\acl_lease.$Mutant.rs" -Destination $target -Force
(Get-Item $target).LastWriteTime = Get-Date
L ("APPLIED=" + $Mutant)
L ("TARGET_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower())

Push-Location 'C:\f28h2-repo'
# Force the rebuild in its own step so compilation is observable separately.
$b = (& cargo build -p wcore-sandbox --tests 2>&1 | Out-String)
$built = ($b -match 'Compiling wcore-sandbox')
L ("MUT_COMPILED=" + $built)

$names = @(
  'backends::appcontainer::appcontainer_acl_lease::tests::dead_owner_unreconcilable_lease_is_reclaimed_not_refused_forever',
  'backends::appcontainer::appcontainer_acl_lease::tests::live_owner_unreconcilable_lease_is_honoured_not_reclaimed',
  'backends::appcontainer::appcontainer_acl_lease::tests::quarantine_directory_does_not_become_a_second_wedge',
  'backends::appcontainer::appcontainer_acl_lease::tests::reclamation_reports_grants_it_could_not_revoke'
)
foreach ($n in $names) {
  $o = (& cargo test -p wcore-sandbox --lib -- --exact $n --test-threads=1 2>&1 | Out-String)
  $mm = [regex]::Match($o, 'test result: (\w+)\. (\d+) passed; (\d+) failed')
  $short = $n.Split(':')[-1]
  if ($mm.Success) {
    L ("MUT2=" + $short + ";result=" + $mm.Groups[1].Value + ";passed=" + $mm.Groups[2].Value + ";failed=" + $mm.Groups[3].Value)
  } else {
    L ("MUT2=" + $short + ";result=UNPARSEABLE")
    L $o
  }
}
Pop-Location

Copy-Item 'C:\f28h2\acl_lease.pristine.rs' -Destination $target -Force
(Get-Item $target).LastWriteTime = Get-Date
L ("RESTORED_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower())
Push-Location 'C:\f28h2-repo'
L ("RESTORED_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)
Pop-Location
L ("finished=" + (Get-Date -Format o))
$rc = 0
if (-not $built) { L 'NOTBUILT'; $rc = 6 }
Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
