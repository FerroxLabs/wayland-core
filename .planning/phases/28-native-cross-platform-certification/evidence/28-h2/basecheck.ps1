# Lane 28-h2 — attribute the 3 bwrap reds and the 4 clippy warnings.
# They are claimed pre-existing and environmental; this MEASURES that at the
# lane base rather than asserting it, then returns to the lane HEAD and rebuilds
# the release binary so the live product evidence is taken at HEAD, not at an
# intermediate commit.
$ErrorActionPreference = 'Continue'
$log    = 'C:\f28h2\basecheck.log'
$status = 'C:\f28h2\basecheck.status'
Remove-Item -Force -ErrorAction SilentlyContinue $status
Set-Content -Path $log -Value ("F28H2 BASECHECK started=" + (Get-Date -Format o)) -Encoding utf8
function L($m) { Add-Content -Path $log -Value $m -Encoding utf8 }
$env:CARGO_TARGET_DIR = 'C:\f28h2-target'
$env:WAYLAND_SANDBOX_LIVE_WINDOWS = '1'

Push-Location 'C:\f28h2-repo'
& git checkout --detach 12fc794f08012333adc7c864cdeff95540c0f014 2>&1 | Out-Null
L ("BASE_SHA=" + (& git rev-parse HEAD).Trim())
L ("BASE_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)

$c = (& cargo clippy -p wcore-sandbox --all-targets 2>&1 | Out-String)
L ("BASE_CLIPPY_WARNINGS=" + ([regex]::Matches($c, 'warning: ')).Count)
L ("BASE_CLIPPY_UNUSED_IMPORT=" + ([regex]::Matches($c, 'unused import')).Count)

foreach ($n in @(
  'backends::bwrap::tests::required_live_bwrap_admission',
  'backends::bwrap::tests::required_live_bwrap_hard_containment_mint_and_drift',
  'backends::bwrap::tests::required_live_bwrap_retained_cwd_enforcement')) {
  $o = (& cargo test -p wcore-sandbox --lib -- --ignored --exact $n --test-threads=1 2>&1 | Out-String)
  # Take the LAST summary line: nested helper processes splice their own
  # summaries into this stream, and the FIRST match is the child's, not ours.
  $ms = [regex]::Matches($o, 'test result: (\w+)\. (\d+) passed; (\d+) failed')
  $short = $n.Split(':')[-1]
  if ($ms.Count -gt 0) {
    $last = $ms[$ms.Count - 1]
    L ("BASE_BWRAP=" + $short + ";result=" + $last.Groups[1].Value + ";passed=" + $last.Groups[2].Value + ";failed=" + $last.Groups[3].Value)
  } else {
    L ("BASE_BWRAP=" + $short + ";result=UNPARSEABLE")
  }
}

& git checkout --detach 3f3f93dc8dc64c847ad5ab0955837d3e91d44d1a 2>&1 | Out-Null
L ("HEAD_SHA=" + (& git rev-parse HEAD).Trim())
L ("HEAD_DIRTY=" + ((& git status --porcelain) | Measure-Object).Count)
& cargo build --release -p wcore-cli 2>&1 | ForEach-Object { Add-Content -Path $log -Value ("BUILD: " + $_) -Encoding utf8 }
$exe = 'C:\f28h2-target\release\wayland-core.exe'
L ("HEAD_EXE_SHA256=" + (Get-FileHash -Algorithm SHA256 -Path $exe).Hash.ToLower())
Pop-Location
L ("finished=" + (Get-Date -Format o))
Set-Content -Path $status -Value "WLRC=0" -Encoding utf8
Add-Content -Path $status -Value "WLDONE" -Encoding utf8
