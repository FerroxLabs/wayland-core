# 28-adj — independent mutation M3.
#
# Hypothesis: the residual-grant DISCLOSURE branch of reclamation_report() is
# guarded by no assertion, so a mutant that deletes it — telling the operator
# "nothing was left behind" when un-revokable ACL grants remain — stays green.
#
# Guards carried over from the traps this program has measured:
#   * stamp LastWriteTime AND assert `Compiling wcore-sandbox` appeared, or the
#     run is graded NOTBUILT (a stale binary reproduced a previous mutant here);
#   * ENUMERATE the test names with --list first and assert the expected count,
#     because an --exact filter matching no name exits 0 having run nothing;
#   * take the LAST `test result:` match, because a nested child test process
#     splices its own summary into the parent stream;
#   * write WLRC/WLDONE to a status file the caller reads back separately,
#     because every non-zero exit collapses to 1 across ssh+PowerShell.
$ErrorActionPreference = 'Continue'
$target = 'C:\f28h2-repo\crates\wcore-sandbox\src\backends\appcontainer\acl_lease.rs'
$work   = 'C:\f28adj'
$log    = "$work\m3.log"
$status = "$work\m3.status"
New-Item -ItemType Directory -Force -Path $work | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28ADJ M3 started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }

$env:CARGO_TARGET_DIR = 'C:\f28h2-target'
Push-Location 'C:\f28h2-repo'
L ("REPO_HEAD=" + (& git rev-parse HEAD))
L ("REPO_DIRTY_BEFORE=" + ((& git status --porcelain) | Measure-Object).Count)
Pop-Location

# Preserve the pristine file ourselves; never rely on git to put it back.
Copy-Item $target -Destination "$work\acl_lease.pristine.rs" -Force
$pristine = (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower()
L ("PRISTINE_SHA256=" + $pristine)

# --- Enumerate the tests BEFORE filtering, so a typo cannot pass silently. ---
Push-Location 'C:\f28h2-repo'
$listOut = (& cargo test -p wcore-sandbox --lib -- --list 2>&1 | Out-String)
Pop-Location
$wanted = @(
  'dead_owner_unreconcilable_lease_is_reclaimed_not_refused_forever',
  'live_owner_unreconcilable_lease_is_honoured_not_reclaimed',
  'quarantine_directory_does_not_become_a_second_wedge',
  'reclamation_reports_grants_it_could_not_revoke',
  'a_leaked_test_lease_is_diagnosed_by_name'
)
# Split and TRIM: the previous attempt anchored on `$` and every line carries a
# trailing CR, so all five filters resolved to zero hits and the guard aborted.
# That abort is the guard working -- without it, five --exact filters matching
# nothing would each have exited 0 reporting `ok`.
$listLines = @()
foreach ($ln in ($listOut -split "`r?`n")) { $listLines += $ln.Trim() }
$resolved = @()
foreach ($w in $wanted) {
  $hits = @($listLines | Where-Object { $_ -like "*::${w}: test" })
  if ($hits.Count -eq 1) {
    $full = $hits[0].Substring(0, $hits[0].Length - ': test'.Length)
    $resolved += $full
    L ("RESOLVED=" + $full)
  } else {
    L ("RESOLVE_FAILED=" + $w + ";hits=" + $hits.Count)
  }
}
L ("RESOLVED_COUNT=" + $resolved.Count + ";EXPECTED=5")
if ($resolved.Count -ne 5) {
  L 'ABORT_NAME_RESOLUTION'
  Set-Content -Path $status -Value "WLRC=7" -Encoding utf8
  Add-Content -Path $status -Value "WLDONE" -Encoding utf8
  exit 0
}

# --- Apply M3. ---
Copy-Item "$work\acl_lease.m3.rs" -Destination $target -Force
(Get-Item $target).LastWriteTime = Get-Date
L ("APPLIED_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower())

Push-Location 'C:\f28h2-repo'
$b = (& cargo build -p wcore-sandbox --tests 2>&1 | Out-String)
$built = ($b -match 'Compiling wcore-sandbox')
L ("MUT_COMPILED=" + $built)

if ($built) {
  foreach ($n in $resolved) {
    $o = (& cargo test -p wcore-sandbox --lib -- --exact $n --test-threads=1 2>&1 | Out-String)
    $mm = [regex]::Matches($o, 'test result: (\w+)\. (\d+) passed; (\d+) failed')
    $short = $n.Split(':')[-1]
    if ($mm.Count -gt 0) {
      $last = $mm[$mm.Count - 1]
      L ("M3=" + $short + ";result=" + $last.Groups[1].Value + ";passed=" + $last.Groups[2].Value + ";failed=" + $last.Groups[3].Value + ";summaries=" + $mm.Count)
    } else {
      L ("M3=" + $short + ";result=UNPARSEABLE")
      L $o
    }
  }
  # And the whole lib suite once, to see whether ANY other test guards it.
  $full = (& cargo test -p wcore-sandbox --lib -- --test-threads=1 2>&1 | Out-String)
  $fm = [regex]::Matches($full, 'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored')
  if ($fm.Count -gt 0) {
    $lf = $fm[$fm.Count - 1]
    L ("M3_FULLSUITE=result=" + $lf.Groups[1].Value + ";passed=" + $lf.Groups[2].Value + ";failed=" + $lf.Groups[3].Value + ";ignored=" + $lf.Groups[4].Value + ";summaries=" + $fm.Count)
  } else {
    L 'M3_FULLSUITE=UNPARSEABLE'
  }
  foreach ($line in ($full -split "`r?`n")) {
    if ($line -match 'FAILED|panicked at') { L ("M3_FULLSUITE_LINE=" + $line.Trim()) }
  }
}
Pop-Location

# --- Restore, and PROVE the restore. ---
Copy-Item "$work\acl_lease.pristine.rs" -Destination $target -Force
(Get-Item $target).LastWriteTime = Get-Date
$restored = (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower()
L ("RESTORED_SHA256=" + $restored)
L ("RESTORE_MATCHES_PRISTINE=" + ($restored -eq $pristine))
Push-Location 'C:\f28h2-repo'
L ("REPO_DIRTY_AFTER=" + ((& git status --porcelain) | Measure-Object).Count)
Pop-Location
L ("finished=" + (Get-Date -Format o))

$rc = 0
if (-not $built) { $rc = 6 }
if ($restored -ne $pristine) { $rc = 9 }
Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
