# Lane 28-h2 — wcore-sandbox unit tests on real Windows.
#
# Two traps this is built against:
#  * a suite can exit 0 having run ZERO tests, so the EXECUTED COUNT is read
#    back and no figure here comes from an exit status;
#  * a filter that matches no test name also exits 0, so each wedge test is
#    additionally run BY EXACT NAME and asserted to have run exactly 1.
# Live Windows runs serially: parallel has produced unstable splits here.
param([string]$Tag = 'after')
$ErrorActionPreference = 'Continue'
$log    = "C:\f28h2\unittest-$Tag.log"
$status = "C:\f28h2\unittest-$Tag.status"
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28H2 UNITTEST tag=$Tag started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }

$env:CARGO_TARGET_DIR = 'C:\f28h2-target'
Push-Location 'C:\f28h2-repo'
L ("SRC_SHA=" + (& git rev-parse HEAD).Trim())
L ("SRC_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)

# --- whole --lib target, by TARGET not by filter ---------------------------
$out = (& cargo test -p wcore-sandbox --lib -- --test-threads=1 2>&1 | Out-String)
L '===== FULL --lib RUN ====='
L $out
$m = [regex]::Match($out, 'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored')
if ($m.Success) {
  L ("LIB_RESULT=" + $m.Groups[1].Value + ";passed=" + $m.Groups[2].Value +
     ";failed=" + $m.Groups[3].Value + ";ignored=" + $m.Groups[4].Value)
} else {
  L 'LIB_RESULT=UNPARSEABLE'
}

# --- each wedge test BY EXACT NAME, asserting it really executed -----------
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
    L ("NAMED=" + $short + ";result=" + $mm.Groups[1].Value + ";passed=" + $mm.Groups[2].Value + ";failed=" + $mm.Groups[3].Value)
  } else {
    L ("NAMED=" + $short + ";result=UNPARSEABLE")
    L $o
  }
}
Pop-Location
L ("finished=" + (Get-Date -Format o))
Set-Content -Path $status -Value "WLRC=0" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
