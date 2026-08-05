# Lane 28-adj2 — full unit verification, and the M3 re-run that F-28-ADJ-001 demands.
#
# Guards carried over, each earned by a gate that silently passed before:
#  * resolve every test name against `--list` FIRST and assert the count, so a
#    filter that matches nothing cannot masquerade as a pass;
#  * take the LAST `test result:` line -- nested helper processes splice their own;
#  * stamp LastWriteTime after Copy-Item AND assert `Compiling wcore-sandbox`,
#    because Copy-Item preserves mtime and cargo will silently reuse a stale binary;
#  * `--list` is matched WITHOUT a `$` anchor: the adjudicator's own instrument
#    lost votes to trailing CRs on that anchor.
param([string]$Sha = '')
$ErrorActionPreference = 'Continue'
$log    = 'C:\f28h2\adj2-verify.log'
$status = 'C:\f28h2\adj2-verify.status'
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28ADJ2 VERIFY started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }
$env:CARGO_TARGET_DIR = 'C:\f28h2-target'
$target = 'C:\f28h2-repo\crates\wcore-sandbox\src\backends\appcontainer\acl_lease.rs'

Push-Location 'C:\f28h2-repo'
if ($Sha) {
  & git fetch origin lane/28-adj2 2>&1 | Out-Null
  & git checkout --detach $Sha 2>&1 | Out-Null
}
L ("SRC_SHA=" + (& git rev-parse HEAD).Trim())
L ("SRC_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)

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

# ---- resolve names against --list; a name that resolves to nothing is fatal ----
$listing = (& cargo test -p wcore-sandbox --lib -- --list 2>&1 | Out-String)
$resolved = @{}
foreach ($n in $names) {
  # No `$` anchor: trailing CR would kill the match.
  $m = [regex]::Match($listing, '(?m)^(\S*' + [regex]::Escape($n) + ')\s*:\s*test')
  if ($m.Success) { $resolved[$n] = $m.Groups[1].Value; L ("RESOLVED=" + $m.Groups[1].Value) }
  else { L ("RESOLVE_FAILED=" + $n) }
}
L ("RESOLVED_COUNT=" + $resolved.Count + ";EXPECTED=" + $names.Count)

function Summary($text) {
  $ms = [regex]::Matches($text, 'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored')
  if ($ms.Count -eq 0) { return $null }
  return $ms[$ms.Count - 1]
}

function RunNamed($tagLabel) {
  foreach ($n in $names) {
    if (-not $resolved.ContainsKey($n)) { L ("$tagLabel=" + $n + ";result=UNRESOLVED"); continue }
    $o = (& cargo test -p wcore-sandbox --lib -- --exact $resolved[$n] --test-threads=1 2>&1 | Out-String)
    $s = Summary $o
    if ($s) { L ("$tagLabel=" + $n + ";result=" + $s.Groups[1].Value + ";passed=" + $s.Groups[2].Value + ";failed=" + $s.Groups[3].Value) }
    else    { L ("$tagLabel=" + $n + ";result=UNPARSEABLE") }
  }
}

# ---- pristine ----------------------------------------------------------------
$full = (& cargo test -p wcore-sandbox --lib -- --test-threads=1 2>&1 | Out-String)
$s = Summary $full
if ($s) { L ("PRISTINE_FULL=result=" + $s.Groups[1].Value + ";passed=" + $s.Groups[2].Value + ";failed=" + $s.Groups[3].Value + ";ignored=" + $s.Groups[4].Value) }
else    { L 'PRISTINE_FULL=UNPARSEABLE' }
RunNamed 'PRISTINE'

Copy-Item $target -Destination 'C:\f28h2\adj2-pristine.rs' -Force
L ("PRISTINE_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower())
Pop-Location
L ("finished=" + (Get-Date -Format o))
Set-Content -Path $status -Value "WLRC=0" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
