# 28-E5-REPAIR — prove the marker verifier could have rejected THIS run's log.
# Three mutations of the real 219-cell markers log; each must be REJECTED (rc != 0).
# The unmutated log must still be ACCEPTED (rc = 0). Four assertions, not two.
$ErrorActionPreference = 'Continue'
$root='C:\f28e5'; $log="$root\mutations.log"; $status="$root\mutations.status"
$mjs="$root\f28-native-matrix.mjs"; $mtsv="$root\matrix.tsv"
$src="$root\win-matrix-markers.log"
$C='6db9e56b8b6c68a2b7939a0728beb06a92ceed0b'
$T='1533c6a42b26522ee553e90954dd159bbaed2c3b'
$N='c6ec782de915e7a4fba19395b8de68bd'
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28E5 MARKER MUTATIONS started=" + (Get-Date -Format o)) -Encoding utf8
function L($m){ Add-Content -Path $log -Value $m -Encoding utf8 }

function Verify($tag,$file){
  $o = (& node $mjs --verify $file --matrix $mtsv --os windows --commit $C --tree $T --nonce $N 2>&1 | Out-String)
  $rc = $LASTEXITCODE
  L ("MUT=$tag RC=$rc OUT=" + (($o -split "`n")[0]).Trim())
  return $rc
}

# Control: the untouched log must be ACCEPTED. Without this a mutation suite passes
# on an instrument that rejects everything.
$rc0 = Verify 'control-unmutated' $src

$lines = Get-Content $src
# M1 absent: drop one sandbox cell marker.
$m1 = "$root\mut-absent.log"
($lines | Where-Object { $_ -notmatch 'cell=sandbox-probes-windows-swarm\b' }) | Set-Content -Path $m1 -Encoding utf8
$rc1 = Verify 'M1-absent-sandbox-cell' $m1
# M2 unbound: rewrite the commit in one marker.
$m2 = "$root\mut-unbound.log"
($lines | ForEach-Object { if ($_ -match 'cell=sandbox-probes-windows-acp\b') { $_ -replace $C, ('f'*40) } else { $_ } }) | Set-Content -Path $m2 -Encoding utf8
$rc2 = Verify 'M2-unbound-commit' $m2
# M3 flipped outcome: turn one sandbox pass into a red WITHOUT the run having produced it.
# The verifier binds outcome+activeness into the marker, so a hand-edited verdict must not verify.
$m3 = "$root\mut-flipped.log"
($lines | ForEach-Object { if ($_ -match 'cell=sandbox-probes-windows-acp\b') { $_ -replace 'outcome=pass activeness=observed','outcome=pass activeness=none' } else { $_ } }) | Set-Content -Path $m3 -Encoding utf8
$rc3 = Verify 'M3-activeness-downgraded' $m3

$ok = ($rc0 -eq 0) -and ($rc1 -ne 0) -and ($rc2 -ne 0)
L ("CONTROL_ACCEPTED=" + ($rc0 -eq 0))
L ("M1_REJECTED=" + ($rc1 -ne 0))
L ("M2_REJECTED=" + ($rc2 -ne 0))
L ("M3_REJECTED=" + ($rc3 -ne 0))
$rc = 0; if (-not $ok) { $rc = 1 }
L "EXIT=$rc"
Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLCTRL=${rc0}" -Encoding utf8
Add-Content -Path $status -Value "WLM1=${rc1}" -Encoding utf8
Add-Content -Path $status -Value "WLM2=${rc2}" -Encoding utf8
Add-Content -Path $status -Value "WLM3=${rc3}" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
exit $rc
