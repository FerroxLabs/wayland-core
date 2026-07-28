# Lane 28-adj2 — clippy + live AppContainer acceptance at lane HEAD.
# Only --lib runs under WAYLAND_SANDBOX_LIVE_WINDOWS: tests/ compile without
# cfg(test) and lease into the REAL %LOCALAPPDATA% (finding F-4).
# Takes the LAST `test result:` line -- nested helpers splice their own summaries.
$ErrorActionPreference = 'Continue'
$log    = 'C:\f28h2\adj2-final.log'
$status = 'C:\f28h2\adj2-final.status'
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28ADJ2 FINAL started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }
$env:CARGO_TARGET_DIR = 'C:\f28h2-target'
Push-Location 'C:\f28h2-repo'
L ("SRC_SHA=" + (& git rev-parse HEAD).Trim())
L ("SRC_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)

$c = (& cargo clippy -p wcore-sandbox --all-targets 2>&1 | Out-String)
L '===== CLIPPY ====='; L $c
L ("CLIPPY_WARNINGS=" + ([regex]::Matches($c, 'warning: ')).Count)
L ("CLIPPY_ERRORS=" + ([regex]::Matches($c, '(?m)^error(\[|:)')).Count)

$env:WAYLAND_SANDBOX_LIVE_WINDOWS = '1'
$o = (& cargo test -p wcore-sandbox --lib -- --ignored --test-threads=1 2>&1 | Out-String)
L '===== LIVE ACCEPTANCE ====='; L $o
$ms = [regex]::Matches($o, 'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored')
if ($ms.Count -gt 0) {
  $s = $ms[$ms.Count - 1]
  L ("LIVE_RESULT=result=" + $s.Groups[1].Value + ";passed=" + $s.Groups[2].Value + ";failed=" + $s.Groups[3].Value + ";ignored=" + $s.Groups[4].Value + ";summaries=" + $ms.Count)
} else { L 'LIVE_RESULT=UNPARSEABLE' }
Pop-Location
L ("finished=" + (Get-Date -Format o))
Set-Content -Path $status -Value "WLRC=0" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
