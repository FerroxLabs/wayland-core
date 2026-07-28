# Lane 28-h2 — clippy + the live AppContainer acceptance tests.
#
# ONLY the --lib target is run under WAYLAND_SANDBOX_LIVE_WINDOWS. Integration
# tests under tests/ compile WITHOUT cfg(test) and therefore lease into the
# REAL %LOCALAPPDATA% directory; running them here is how finding F-4 poisoned
# production leases, and this lane will not regress that.
$ErrorActionPreference = 'Continue'
$log    = 'C:\f28h2\final.log'
$status = 'C:\f28h2\final.status'
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28H2 FINAL started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }
$env:CARGO_TARGET_DIR = 'C:\f28h2-target'
Push-Location 'C:\f28h2-repo'
L ("SRC_SHA=" + (& git rev-parse HEAD).Trim())
L ("SRC_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)

$c = (& cargo clippy -p wcore-sandbox --all-targets 2>&1 | Out-String)
L '===== CLIPPY ====='
L $c
$warn = ([regex]::Matches($c, 'warning: ')).Count
$err  = ([regex]::Matches($c, '^error(\[|:)', 'Multiline')).Count
L ("CLIPPY_WARNINGS=" + $warn + ";CLIPPY_ERRORS=" + $err)

# Live acceptance: these are #[ignore]d and need BOTH --ignored and the env var.
# A run that forgets either exits 0 having executed ZERO tests, so the executed
# count is read back and is the only thing graded.
$env:WAYLAND_SANDBOX_LIVE_WINDOWS = '1'
$o = (& cargo test -p wcore-sandbox --lib -- --ignored --test-threads=1 2>&1 | Out-String)
L '===== LIVE ACCEPTANCE (--lib, --ignored) ====='
L $o
$m = [regex]::Match($o, 'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored')
if ($m.Success) {
  L ("LIVE_RESULT=" + $m.Groups[1].Value + ";passed=" + $m.Groups[2].Value +
     ";failed=" + $m.Groups[3].Value + ";ignored=" + $m.Groups[4].Value)
} else {
  L 'LIVE_RESULT=UNPARSEABLE'
}
Pop-Location
L ("finished=" + (Get-Date -Format o))
Set-Content -Path $status -Value "WLRC=0" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
