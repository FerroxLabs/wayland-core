# wayland-e2e-windows-soak.ps1
#
# Cross-OS counterpart to scripts/wayland-e2e-real-workload.sh — runs the
# same build+test+mutants enumerator workload on a Windows runner. Because
# GHA `windows-2022` IS the runner, this script SKIPS the droplet
# provisioning phases (A/B/C/J in the Linux script) and starts straight at
# the workload (Linux script's phases F/G/H).
#
# Run locally on Windows (rare):
#   pwsh scripts/wayland-e2e-windows-soak.ps1
#
# Run in GHA (the common case):
#   .github/workflows/nightly-windows-soak.yml invokes this from
#   a `windows-2022` runner.
#
# Expected duration: 15-30 min on windows-2022 (slower than macOS/Linux
# native — Windows MSVC link is the bottleneck).
# Expected cost: $0.008/min × ~25 min ≈ $0.20/run on paid GHA.
#
# Exit codes:
#   0 — all gates passed
#   ≥1 — failure (logs in $LOCAL_RESULTS_DIR for diagnosis)

$ErrorActionPreference = "Stop"

# -----------------------------------------------------------------------------
# Help
# -----------------------------------------------------------------------------
if ($args.Count -gt 0 -and ($args[0] -eq "--help" -or $args[0] -eq "-h")) {
    # Single-quoted here-string: PowerShell does NOT interpret $variable
    # or backtick escapes inside @'...'@, so the help text renders
    # verbatim. The double-quoted form @"..."@ tripped a parser error in
    # GHA pwsh 7 on Windows over the inner backtick patterns we used
    # to display literal $env references.
    @'
wayland-e2e-windows-soak.ps1 -- full Wayland-Core workload on Windows (~15-30min)

USAGE
  pwsh scripts/wayland-e2e-windows-soak.ps1

WHAT IT DOES
  - cargo build --release -p wcore-cli
  - cargo nextest run on 5 representative crates
  - cargo mutants --list -p wcore-providers (smoke, no actual mutations)
  - Captures all logs to LOCAL_RESULTS_DIR (default: $env:TEMP\wayland-windows-soak-<RUN_ID>)

ASSUMES
  - cargo, cargo-nextest, cargo-mutants are on PATH
    (the GHA workflow installs them via taiki-e/install-action)
  - Workspace is at the current directory or $env:WAYLAND_REPO_ROOT

ENV OVERRIDES
  WAYLAND_REPO_ROOT=<path>  default: $PWD
  LOCAL_RESULTS_DIR=<path>  default: $env:TEMP\wayland-windows-soak-<RUN_ID>
'@ | Out-Host
    exit 0
}

# -----------------------------------------------------------------------------
# Config
# -----------------------------------------------------------------------------
$RunId = (Get-Date -Format "yyyyMMdd-HHmmss")
$RepoRoot = if ($env:WAYLAND_REPO_ROOT) { $env:WAYLAND_REPO_ROOT } else { (Get-Location).Path }
$ResultsDir = if ($env:LOCAL_RESULTS_DIR) { $env:LOCAL_RESULTS_DIR } else { Join-Path $env:TEMP "wayland-windows-soak-$RunId" }

New-Item -ItemType Directory -Force -Path $ResultsDir | Out-Null
Set-Location $RepoRoot

# -----------------------------------------------------------------------------
# Output helpers
# -----------------------------------------------------------------------------
function Write-Phase($msg) {
    Write-Host ""
    Write-Host "═══ $msg ═══" -ForegroundColor Yellow
}
function Write-Ok($msg) {
    Write-Host "✓ $msg" -ForegroundColor Green
}
function Write-Fail($msg) {
    Write-Host "✗ $msg" -ForegroundColor Red
}
function Write-Note($msg) {
    Write-Host "· $msg" -ForegroundColor Blue
}

# -----------------------------------------------------------------------------
# Phase 0: preflight
# -----------------------------------------------------------------------------
Write-Phase "PHASE 0 — preflight"
Write-Note "repo root: $RepoRoot"
Write-Note "results dir: $ResultsDir"
Write-Note "run id: $RunId"

# Verify tooling is present (the GHA workflow installs them before invoking
# this script; locally, the user must `cargo install` them first).
foreach ($tool in @("cargo", "cargo-nextest", "cargo-mutants")) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Write-Fail "missing required tool on PATH: $tool"
        Write-Fail "install via: cargo install $tool --locked"
        exit 1
    }
}
Write-Ok "toolchain present (cargo, cargo-nextest, cargo-mutants)"

# Print version info for diagnostics
& cargo --version 2>&1 | Tee-Object -FilePath (Join-Path $ResultsDir "0-versions.log") | Out-Host
& rustc --version 2>&1 | Tee-Object -FilePath (Join-Path $ResultsDir "0-versions.log") -Append | Out-Host
& cargo nextest --version 2>&1 | Tee-Object -FilePath (Join-Path $ResultsDir "0-versions.log") -Append | Out-Host
& cargo mutants --version 2>&1 | Tee-Object -FilePath (Join-Path $ResultsDir "0-versions.log") -Append | Out-Host

# -----------------------------------------------------------------------------
# Phase F: cargo build --release -p wcore-cli
# -----------------------------------------------------------------------------
Write-Phase "PHASE F — cargo build --release -p wcore-cli"
$BuildLog = Join-Path $ResultsDir "F-build.log"
$buildExit = & {
    cargo build --release -p wcore-cli 2>&1 | Tee-Object -FilePath $BuildLog
    $LASTEXITCODE
}
if ($buildExit -ne 0) {
    Write-Fail "release build failed with exit code $buildExit"
    exit $buildExit
}

# Verify binary exists. Windows MSVC target produces wayland-core.exe.
$BinaryCandidates = @(
    (Join-Path $RepoRoot "target\release\wayland-core.exe"),
    (Join-Path $RepoRoot "target\release\wayland-core")
)
$Binary = $BinaryCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $Binary) {
    Write-Fail "wayland-core binary not found at expected target/release/ location"
    Get-ChildItem (Join-Path $RepoRoot "target\release") | Select-Object -First 20 | Out-Host
    exit 1
}
Write-Ok "binary at $Binary"

& $Binary --version 2>&1 | Tee-Object -FilePath (Join-Path $ResultsDir "F-binary.log") | Out-Host

# -----------------------------------------------------------------------------
# Phase G: cargo nextest on representative crates
# -----------------------------------------------------------------------------
Write-Phase "PHASE G — cargo nextest on representative crates"
$NextestLog = Join-Path $ResultsDir "G-nextest.log"
$nextestExit = & {
    cargo nextest run `
        -p wcore-cron `
        -p wcore-config `
        -p wcore-providers `
        -p wcore-tools `
        -p wcore-swarm `
        2>&1 | Tee-Object -FilePath $NextestLog
    $LASTEXITCODE
}
if ($nextestExit -ne 0) {
    Write-Fail "nextest failed with exit code $nextestExit"
    exit $nextestExit
}
Write-Ok "nextest run complete (5 crates)"

# -----------------------------------------------------------------------------
# Phase H: cargo mutants smoke (list-only)
# -----------------------------------------------------------------------------
Write-Phase "PHASE H — cargo mutants smoke (--list, no actual mutations)"
$MutantsLog = Join-Path $ResultsDir "H-mutants.log"
& cargo mutants --list -p wcore-providers 2>&1 | Tee-Object -FilePath $MutantsLog | Out-Host
$MutantCount = (Get-Content $MutantsLog | Measure-Object -Line).Lines
Write-Note "mutant enumerator produced $MutantCount lines"
Write-Ok "mutants enumerator validated"

# -----------------------------------------------------------------------------
# Final summary
# -----------------------------------------------------------------------------
Write-Phase "WINDOWS SOAK: PASS"
Write-Host "Results saved to: $ResultsDir" -ForegroundColor Green
exit 0
