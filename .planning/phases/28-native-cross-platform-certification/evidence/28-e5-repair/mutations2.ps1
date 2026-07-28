# 28-E5-REPAIR — marker-mutation control, REPAIRED.
#
# The first version of this script was itself defective and is the reason this one exists.
# It wrote its mutants with `Set-Content`, which emits CRLF; the verifier rejects any CR byte
# under its LF-only authority grammar, so all three mutants were rejected for the WRONG reason
# and the suite would have reported a healthy instrument even if every mutation were invisible.
# That is the eleventh-instance defect class this program keeps finding: an instrument carrying
# the defect it hunts. Repaired here rather than written up and left (§6b-ii).
#
# The repair is byte-exact LF-only writing, and it carries THREE assertions, not two:
#   A. the untouched log is ACCEPTED                       (the instrument is not reject-everything)
#   B. a NULL mutation — rewritten through the same writer, content identical — is ACCEPTED
#      <- this is the assertion the BROKEN writer fails, so it is what proves the repair did
#         anything at all
#   C. each real mutation is REJECTED, and its rejection reason is NOT the CR-byte message
$ErrorActionPreference = 'Continue'
$root='C:\f28e5'; $log="$root\mutations2.log"; $status="$root\mutations2.status"
$mjs="$root\f28-native-matrix.mjs"; $mtsv="$root\matrix.tsv"
$src="$root\win-matrix-markers.log"
$C='6db9e56b8b6c68a2b7939a0728beb06a92ceed0b'
$T='1533c6a42b26522ee553e90954dd159bbaed2c3b'
$N='c6ec782de915e7a4fba19395b8de68bd'
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28E5 MARKER MUTATIONS v2 (LF-exact) started=" + (Get-Date -Format o)) -Encoding utf8
function L($m){ Add-Content -Path $log -Value $m -Encoding utf8 }

# LF-only, no BOM, trailing LF — the same grammar the runner emits.
function WriteLf($path, $lines) {
  $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($path, (($lines -join "`n") + "`n"), $utf8NoBom)
}

$crRejects = 0
function Verify($tag,$file){
  $o = (& node $mjs --verify $file --matrix $mtsv --os windows --commit $C --tree $T --nonce $N 2>&1 | Out-String)
  $rc = $LASTEXITCODE
  $first = (($o -split "`n") | Where-Object { $_.Trim() } | Select-Object -First 1)
  $isCr = ($o -match 'CR byte in authority artifact')
  if ($isCr) { $script:crRejects++ }
  L ("MUT=$tag RC=$rc CR_REASON=$isCr OUT=" + $first.Trim())
  return @($rc, $isCr)
}

# Read the source as raw bytes and split on LF so no CR is ever introduced by the reader.
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$raw = [System.IO.File]::ReadAllText($src, $utf8NoBom)
$lines = $raw.TrimEnd("`n") -split "`n"
L ("SRC_LINES=" + $lines.Count)
L ("SRC_HAS_CR=" + ($raw.Contains("`r")))

# A. untouched
$a = Verify 'A-control-untouched' $src

# B. NULL mutation — identical content through the repaired writer.
$mb = "$root\mut2-null.log"
WriteLf $mb $lines
$b = Verify 'B-null-mutation-lf-writer' $mb

# C1 absent
$m1 = "$root\mut2-absent.log"
WriteLf $m1 ($lines | Where-Object { $_ -notmatch 'cell=sandbox-probes-windows-swarm ' })
$c1 = Verify 'C1-absent-sandbox-cell' $m1
# C2 unbound commit
$m2 = "$root\mut2-unbound.log"
WriteLf $m2 ($lines | ForEach-Object { if ($_ -match 'cell=sandbox-probes-windows-acp ') { $_ -replace $C, ('f'*40) } else { $_ } })
$c2 = Verify 'C2-unbound-commit' $m2
# C3 duplicate
$m3 = "$root\mut2-duplicate.log"
$dupSrc = ($lines | Where-Object { $_ -match 'cell=sandbox-probes-windows-acp ' } | Select-Object -First 1)
$dup = @(); foreach ($l in $lines) { $dup += $l; if ($l -match 'cell=sandbox-probes-windows-acp ') { $dup += $dupSrc } }
WriteLf $m3 $dup
$c3 = Verify 'C3-duplicate-marker' $m3

$aOk  = ($a[0] -eq 0)
$bOk  = ($b[0] -eq 0)                                   # the assertion the broken writer failed
$cOk  = ($c1[0] -ne 0) -and ($c2[0] -ne 0) -and ($c3[0] -ne 0)
$cWhy = (-not $c1[1]) -and (-not $c2[1]) -and (-not $c3[1])   # rejected for the RIGHT reason
L ("A_CONTROL_ACCEPTED=" + $aOk)
L ("B_NULL_MUTATION_ACCEPTED=" + $bOk)
L ("C_ALL_MUTANTS_REJECTED=" + $cOk)
L ("C_REJECTED_FOR_THE_RIGHT_REASON=" + $cWhy)
$rc = 0
if (-not ($aOk -and $bOk -and $cOk -and $cWhy)) { $rc = 1 }
L "EXIT=$rc"
Set-Content -Path $status -Value "WLRC=${rc}" -Encoding utf8
Add-Content -Path $status -Value "WLA=$($a[0])" -Encoding utf8
Add-Content -Path $status -Value "WLB=$($b[0])" -Encoding utf8
Add-Content -Path $status -Value "WLC1=$($c1[0])" -Encoding utf8
Add-Content -Path $status -Value "WLC2=$($c2[0])" -Encoding utf8
Add-Content -Path $status -Value "WLC3=$($c3[0])" -Encoding utf8
Add-Content -Path $status -Value "WLCRREJECTS=${crRejects}" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
exit $rc
