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
# PHASE L (20A-01 Wiring C) — native Windows live-acceptance ignored set.
# `crates/wcore-sandbox/tests/live_fs_acl.rs` carries twelve Windows-only
# `#[ignore]`d ACL tests and
# `crates/wcore-sandbox/tests/hard_process_containment_windows.rs` carries six
# Windows-only `#[ignore]`d Job-Object tests plus an unselected gate marker.
# Before 20A-01 only two of the ACL twelve, and none of the containment gate
# marker, were named by ANY runner (`scripts/f20-native-windows-proof.ps1`
# selects exactly six canonical targets and its target array is 20A-04's
# verified invariant — it is deliberately NOT extended). Phase L gives the whole
# ignored set of both files a recurring, NON-PROOF execution path here, selected
# at FILE level (`--test <file> --run-ignored all`) so a test added later cannot
# silently fall out of a hand-enumerated list, and with `--no-tests=fail` so an
# empty selector fails closed.
#
# Phase L needs a host whose AppContainer runtime is actually available: the
# hosted `windows-2022` image is a server SKU that reports
# `AppContainerBackend::is_available() == false` (documented in
# .github/workflows/nightly-windows-soak.yml on the f20-windows-candidate job,
# which was moved to self-hosted for exactly this reason). It is therefore
# opt-in via WAYLAND_SOAK_LIVE_ACCEPTANCE and driven by the self-hosted
# `windows-live-acceptance` job in that workflow. When the opt-in is absent the
# phase does NOT run and says so loudly — it is never reported as passed.
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
  - cargo nextest run on 6 representative crates (includes wcore-sandbox)
  - cargo mutants --list -p wcore-providers (smoke, no actual mutations)
  - PHASE L (opt-in): the native Windows live-acceptance ignored set --
    the twelve live_fs_acl ACL tests and the six
    hard_process_containment_windows Job-Object tests plus its gate marker
  - Captures all logs to LOCAL_RESULTS_DIR (default: $env:TEMP\wayland-windows-soak-<RUN_ID>)

ASSUMES
  - cargo, cargo-nextest are on PATH; cargo-mutants too unless
    WAYLAND_SOAK_LIVE_ACCEPTANCE=only
    (the GHA workflow installs them via taiki-e/install-action)
  - Workspace is at the current directory or $env:WAYLAND_REPO_ROOT
  - PHASE L additionally requires an AppContainer-capable Windows client SKU;
    the hosted windows-2022 server image is NOT one

ENV OVERRIDES
  WAYLAND_REPO_ROOT=<path>  default: $PWD
  LOCAL_RESULTS_DIR=<path>  default: $env:TEMP\wayland-windows-soak-<RUN_ID>
  WAYLAND_SOAK_LIVE_ACCEPTANCE=<unset|1|only>  default: unset
                            unset -> PHASE L does not run (reported, never
                                     counted as passed)
                            1     -> PHASE L runs after phases F/G/H
                            only  -> PHASE L runs alone (F/G/H skipped)
'@ | Out-Host
    exit 0
}

# -----------------------------------------------------------------------------
# Config
# -----------------------------------------------------------------------------
$RunId = (Get-Date -Format "yyyyMMdd-HHmmss")
$RepoRoot = if ($env:WAYLAND_REPO_ROOT) { $env:WAYLAND_REPO_ROOT } else { (Get-Location).Path }
$ResultsDir = if ($env:LOCAL_RESULTS_DIR) { $env:LOCAL_RESULTS_DIR } else { Join-Path $env:TEMP "wayland-windows-soak-$RunId" }

# PHASE L opt-in (see the Phase L note in the header). Anything other than the
# two recognised values is a configuration error, not a silent "off" — a typo
# must never degrade the live-acceptance gate into a skip.
$LiveAcceptance = if ($env:WAYLAND_SOAK_LIVE_ACCEPTANCE) { $env:WAYLAND_SOAK_LIVE_ACCEPTANCE.Trim().ToLowerInvariant() } else { "" }
if ($LiveAcceptance -ne "" -and $LiveAcceptance -ne "1" -and $LiveAcceptance -ne "only") {
    Write-Host "unrecognised WAYLAND_SOAK_LIVE_ACCEPTANCE='$LiveAcceptance' (expected unset, '1' or 'only')" -ForegroundColor Red
    exit 1
}

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
# PHASE L — native Windows live-acceptance ignored set (20A-01 Wiring C)
# -----------------------------------------------------------------------------
# Selects the WHOLE ignored set of each file rather than enumerating test names.
# Enumeration is exactly how ten of the twelve live_fs_acl tests fell out of
# every runner in the first place; a file-level selector cannot silently lose a
# test added later. `--no-tests=fail` fails closed when a selector matches
# nothing, matching the discipline f20-native-windows-proof.ps1 already applies.
function Invoke-LiveAcceptancePhase {
    param([string]$ResultsDir)

    Write-Phase "PHASE L — native Windows live-acceptance ignored set"

    # The live-acceptance helper at the top of both test files hard-asserts
    # WAYLAND_SANDBOX_LIVE_WINDOWS == "1". Set it in the trap-safe PowerShell
    # form: the `cmd` form `set VAR=value && ...` appends a TRAILING SPACE that
    # Rust reads verbatim, the assert then fails (or, on a permissive gate,
    # every test silently skips) and the run is vacuous. Prove the value took
    # effect — byte-exactly, delimited — before trusting anything downstream.
    $env:WAYLAND_SANDBOX_LIVE_WINDOWS = '1'
    $observed = $env:WAYLAND_SANDBOX_LIVE_WINDOWS
    Write-Note "WAYLAND_SANDBOX_LIVE_WINDOWS=[$observed] (len=$($observed.Length))"
    if ($observed -cne '1') {
        Write-Fail "live-acceptance flag did not take effect byte-exactly: [$observed]"
        exit 1
    }
    Write-Ok "live-acceptance flag proven effective (exactly '1', no trailing space)"

    # id -> the file-level ignored-set selector that runs it.
    #   live_fs_acl                       -- 12 Windows-only ACL tests. Only
    #                                        one_execution_grant_never_leaks_to_another_identity
    #                                        and granted_path_is_readable_then_revoked
    #                                        were named anywhere before 20A-01;
    #                                        the other ten, including
    #                                        deny_ace_still_blocks_granted_read and
    #                                        normal_sid_only_grant_is_denied, ran nowhere.
    #   hard_process_containment_windows  -- 6 Windows-only Job-Object tests plus
    #                                        native_containment_gate_marker, the gate
    #                                        marker no gate selected. Wired, not deleted.
    $liveSuites = @(
        @{ id = 'live_fs_acl'; args = @('-p', 'wcore-sandbox', '--test', 'live_fs_acl') },
        @{ id = 'hard_process_containment_windows'; args = @('-p', 'wcore-sandbox', '--test', 'hard_process_containment_windows') }
    )

    $failed = @()
    foreach ($suite in $liveSuites) {
        $log = Join-Path $ResultsDir "L-$($suite.id).log"
        $nextestArgs = @('nextest', 'run', '--run-ignored', 'all', '--no-tests=fail', '--no-fail-fast') + $suite.args + @('--nocapture')
        $suiteExit = & {
            cargo @nextestArgs 2>&1 | Tee-Object -FilePath $log
            $LASTEXITCODE
        }
        if ($suiteExit -ne 0) {
            Write-Fail "live-acceptance suite $($suite.id) failed with exit code $suiteExit"
            $failed += $suite.id
        }
        else {
            Write-Ok "live-acceptance suite $($suite.id) passed"
        }
    }

    if ($failed.Count -gt 0) {
        Write-Fail "PHASE L failed: $($failed -join ', ')"
        exit 1
    }
    Write-Ok "PHASE L complete (live_fs_acl + hard_process_containment_windows ignored sets)"
}

# -----------------------------------------------------------------------------
# Phase 0: preflight
# -----------------------------------------------------------------------------
Write-Phase "PHASE 0 — preflight"
Write-Note "repo root: $RepoRoot"
Write-Note "results dir: $ResultsDir"
Write-Note "run id: $RunId"
Write-Note "live-acceptance mode (PHASE L): $(if ($LiveAcceptance) { $LiveAcceptance } else { '<off>' })"

# Verify tooling is present (the GHA workflow installs them before invoking
# this script; locally, the user must `cargo install` them first).
# cargo-mutants backs PHASE H only, so `only` mode does not require it.
$RequiredTools = if ($LiveAcceptance -eq "only") { @("cargo", "cargo-nextest") } else { @("cargo", "cargo-nextest", "cargo-mutants") }
foreach ($tool in $RequiredTools) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Write-Fail "missing required tool on PATH: $tool"
        Write-Fail "install via: cargo install $tool --locked"
        exit 1
    }
}
Write-Ok "toolchain present ($($RequiredTools -join ', '))"

# Print version info for diagnostics
& cargo --version 2>&1 | Tee-Object -FilePath (Join-Path $ResultsDir "0-versions.log") | Out-Host
& rustc --version 2>&1 | Tee-Object -FilePath (Join-Path $ResultsDir "0-versions.log") -Append | Out-Host
& cargo nextest --version 2>&1 | Tee-Object -FilePath (Join-Path $ResultsDir "0-versions.log") -Append | Out-Host
if ($LiveAcceptance -ne "only") {
    & cargo mutants --version 2>&1 | Tee-Object -FilePath (Join-Path $ResultsDir "0-versions.log") -Append | Out-Host
}

# `only` mode runs the live-acceptance surface alone: it exists so the
# AppContainer-capable self-hosted runner can carry PHASE L nightly without
# re-running the whole windows-2022 workload it already covers.
if ($LiveAcceptance -eq "only") {
    Invoke-LiveAcceptancePhase -ResultsDir $ResultsDir
    Write-Phase "WINDOWS LIVE-ACCEPTANCE SOAK: PASS"
    Write-Host "Results saved to: $ResultsDir" -ForegroundColor Green
    exit 0
}

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
# wcore-sandbox joined this list in 20A-01 (Wiring B). It holds 105 of the 155
# Windows-only tests in the workspace — including every retained-handle security
# proof (directory_authority_windows_tests.rs, the appcontainer windows_impl
# module, the acl_lease modules) — and until 20A-01 it was in NO recurring
# automation on any branch: the PR-time Windows leg of ci.yml only fires on
# main, and this soak's crate list did not name it. Its default (non-ignored)
# Windows-only tests run here; its #[ignore]d live-acceptance set runs in
# PHASE L, which needs an AppContainer-capable host. Do not drop a crate from
# this list to shorten the run — a timeout is a finding, not a reason to
# uncover 105 tests again.
$NextestLog = Join-Path $ResultsDir "G-nextest.log"
$nextestExit = & {
    cargo nextest run `
        -p wcore-cron `
        -p wcore-config `
        -p wcore-providers `
        -p wcore-tools `
        -p wcore-swarm `
        -p wcore-sandbox `
        2>&1 | Tee-Object -FilePath $NextestLog
    $LASTEXITCODE
}
if ($nextestExit -ne 0) {
    Write-Fail "nextest failed with exit code $nextestExit"
    exit $nextestExit
}
Write-Ok "nextest run complete (6 crates)"

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
# Phase L: native Windows live-acceptance ignored set (opt-in)
# -----------------------------------------------------------------------------
if ($LiveAcceptance -eq "1") {
    Invoke-LiveAcceptancePhase -ResultsDir $ResultsDir
}
else {
    # Explicitly NOT a pass. Say so loudly so a reader of this log can never
    # mistake the absent phase for a green one.
    Write-Phase "PHASE L — SKIPPED (not run, NOT passed)"
    Write-Note "WAYLAND_SOAK_LIVE_ACCEPTANCE is unset, so the live_fs_acl and"
    Write-Note "hard_process_containment_windows ignored sets did NOT execute in this run."
    Write-Note "They require an AppContainer-capable Windows client SKU; the hosted"
    Write-Note "windows-2022 server image reports AppContainerBackend::is_available() == false."
    Write-Note "The self-hosted 'windows-live-acceptance' job in nightly-windows-soak.yml"
    Write-Note "runs them with WAYLAND_SOAK_LIVE_ACCEPTANCE=only."
}

# -----------------------------------------------------------------------------
# Final summary
# -----------------------------------------------------------------------------
Write-Phase "WINDOWS SOAK: PASS"
Write-Host "Results saved to: $ResultsDir" -ForegroundColor Green
exit 0
