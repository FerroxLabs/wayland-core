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
#   70 — a native command's exit status was not an integer (see the
#        "Fail-closed primitives" block below); the run is untrustworthy
#   71 — a phase executed fewer tests than its floor, or could not prove how
#        many it executed at all
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

ANTI-VACUITY
  Every phase asserts a POSITIVE LOWER BOUND on what it executed. A run that
  executes zero tests, or cannot prove how many it executed, EXITS NONZERO.
  Floors ratchet UP only -- the env overrides below cannot lower them.

ENV OVERRIDES
  WAYLAND_REPO_ROOT=<path>  default: $PWD
  LOCAL_RESULTS_DIR=<path>  default: $env:TEMP\wayland-windows-soak-<RUN_ID>
  WAYLAND_SOAK_MIN_TESTS_G=<n>            default: 100  (PHASE G, 6 crates)
  WAYLAND_SOAK_MIN_MUTANTS_H=<n>          default: 20   (PHASE H enumerator)
  WAYLAND_SOAK_MIN_TESTS_LIVE_FS_ACL=<n>  default: 13   (PHASE L)
  WAYLAND_SOAK_MIN_TESTS_HARD_PROCESS_CONTAINMENT_WINDOWS=<n>
                                          default: 6    (PHASE L)
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
# Fail-closed primitives
# -----------------------------------------------------------------------------
# WHY THIS BLOCK EXISTS — the most dishonest gate this repo has shipped.
#
# On origin/main this soak reported SUCCESS on every scheduled night from
# 2026-07-25 to 2026-07-31 (runs 30149496548, 30193657704, 30251735227,
# 30340579854, 30434098049, 30524554501, 30609877419) while executing ZERO
# tests. Measured on run 30193657704: the uploaded artifact contains exactly two
# files — 0-versions.log and F-build.log — and NO G-nextest.log and NO
# H-mutants.log. F-build.log ends
#   Finished `release` profile [optimized] target(s) in 12m 36s
# so the build SUCCEEDED, yet the job log at 08:08:45 reads
#   ✗ release build failed with exit code    Compiling workspace-hack v0.1.0 (…
# and the run is green.
#
# Two independent PowerShell defects compose to produce that:
#
#  1. ARRAY CAPTURE. `$x = & { native | Tee-Object -FilePath f; $LASTEXITCODE }`
#     returns an ARRAY of (every piped output line + the exit code), because
#     Tee-Object passes each line through to the success stream. `$array -ne 0`
#     is then a FILTER, not a comparison, and its non-empty result is always
#     truthy — so the failure branch is taken on a GREEN build.
#     Measured on pwsh 7.6.3 / Windows 10.0.26200:
#       cmd rc=0   -> System.Object[] (3 elements); ($x -ne 0) yields 2
#                     elements, [bool] = True  -> failure branch taken
#       cmd rc=101 -> System.Object[] (3 elements); failure branch taken
#     Reading $LASTEXITCODE on the line AFTER the pipeline yields System.Int32
#     and branches correctly in both directions. Every call site below does that.
#
#  2. NON-INTEGER EXIT. `exit $array` converts to a process exit code of ZERO.
#     Measured on the same host: a child `pwsh -File` whose last statement is
#     `exit @('   Compiling wcore-cli','    Finished …', 0)` leaves
#     $LASTEXITCODE = 0 in the parent — which is exactly the value GHA reads
#     through its `pwsh` shell wrapper.
#
# Net effect: the script bailed out at the end of PHASE F announcing a failure
# and handed the runner a 0. PHASE G (nextest) and PHASE H (mutants) never ran,
# for a week, behind a green badge.
#
# Defect 1 is closed at each call site. `Exit-Soak` closes defect 2
# structurally. `Assert-TestsExecuted` adds the thing whose absence made the
# whole run vacuous even when it did reach the test phases: a POSITIVE LOWER
# BOUND on the number of tests actually executed.

# The ONLY exit path in this script. Refuses to convert a non-integer status
# into a process exit code, so defect 2 can never silently recur.
function Exit-Soak {
    param([Parameter(Mandatory)][AllowNull()]$Code)

    if ($null -eq $Code -or $Code -is [array] -or $Code -is [string]) {
        $t = if ($null -eq $Code) { '<null>' } else { $Code.GetType().FullName }
        Write-Host "FATAL: Exit-Soak received a non-integer status of type $t." -ForegroundColor Red
        Write-Host "FATAL: refusing to let it collapse to 0. Exiting 70." -ForegroundColor Red
        exit 70
    }
    $rc = 0
    if (-not [int]::TryParse([string]$Code, [ref]$rc)) {
        Write-Host "FATAL: Exit-Soak could not parse '$Code' as an exit code. Exiting 70." -ForegroundColor Red
        exit 70
    }
    # A nonzero intent must never wrap back to 0 through POSIX 8-bit truncation
    # (e.g. 256) or arrive negative.
    if ($rc -ne 0 -and (($rc % 256) -eq 0 -or $rc -lt 0)) { $rc = 1 }
    exit $rc
}

# Validates a captured native exit status and fails closed on anything that is
# not the integer 0. The type check is not paranoia: it is defect 1's signature.
function Assert-NativeExit {
    param(
        [Parameter(Mandatory)][AllowNull()]$Code,
        [Parameter(Mandatory)][string]$What
    )
    if ($null -eq $Code -or $Code -is [array] -or $Code -is [string]) {
        $t = if ($null -eq $Code) { '<null>' } else { $Code.GetType().FullName }
        Write-Fail "$What produced a non-integer exit status of type $t."
        Write-Fail "That is the array-capture defect. Read `$LASTEXITCODE on the line AFTER the pipeline."
        Exit-Soak 70
    }
    if ($Code -ne 0) {
        Write-Fail "$What failed with exit code $Code"
        Exit-Soak $Code
    }
}

# cargo and nextest colourise under CARGO_TERM_COLOR=always (GHA sets it on
# every Windows job), so summary lines arrive wrapped in SGR escapes. Strip them
# before matching, or every count below silently reads zero.
function Get-CleanLogText {
    param([Parameter(Mandatory)][string]$LogPath)
    if (-not (Test-Path -LiteralPath $LogPath)) { return $null }
    $text = Get-Content -LiteralPath $LogPath -Raw
    if ([string]::IsNullOrEmpty($text)) { return $null }
    return ($text -replace "$([char]27)\[[0-9;?]*[ -/]*[@-~]", '')
}

# nextest prints exactly one final `Summary [   1.234s] N tests run: …` line, on
# stderr, which every call site here merges in with 2>&1. Returns -1 when no
# such line exists; callers treat that as a hard failure, because a soak that
# cannot say how many tests it ran has not proved that it ran any.
function Get-NextestExecutedCount {
    param([Parameter(Mandatory)][string]$LogPath)
    $text = Get-CleanLogText -LogPath $LogPath
    if ($null -eq $text) { return -1 }
    $m = [regex]::Matches($text, 'Summary\s+\[[^\]]*\]\s+(\d+)\s+tests?\s+run:')
    if ($m.Count -eq 0) { return -1 }
    return [int]$m[$m.Count - 1].Groups[1].Value
}

function Assert-TestsExecuted {
    param(
        [Parameter(Mandatory)][string]$Phase,
        [Parameter(Mandatory)][string]$LogPath,
        [Parameter(Mandatory)][int]$Minimum
    )
    $count = Get-NextestExecutedCount -LogPath $LogPath
    if ($count -lt 0) {
        Write-Fail "$Phase — no nextest 'Summary [...] N tests run' line in $LogPath."
        Write-Fail "A run that cannot prove how many tests it executed is a VACUOUS run, not a pass."
        Exit-Soak 71
    }
    if ($count -lt $Minimum) {
        Write-Fail "$Phase executed $count tests, below the required floor of $Minimum."
        Write-Fail "ZERO OR TOO FEW TESTS EXECUTED IS A FAILED RUN. It is never a pass."
        Exit-Soak 71
    }
    Write-Ok "$Phase executed $count tests (floor $Minimum)"
}

# `cargo mutants --list` prints one mutant per line as
# `<path>.rs:<line>:<col>: replace … with …`. Counting LOG lines instead — what
# PHASE H used to do, and then discard without ever comparing — counts
# warnings, progress spinners and error text just as happily as mutants.
function Get-MutantListCount {
    param([Parameter(Mandatory)][string]$LogPath)
    $text = Get-CleanLogText -LogPath $LogPath
    if ($null -eq $text) { return -1 }
    return ([regex]::Matches($text, '(?m)^\s*\S*\.rs:\d+:\d+:\s')).Count
}

# Ratchet-only floor. An operator may RAISE a floor (after measuring a real
# green run) but never lower one: the entire point of these gates is that they
# cannot be talked back down towards zero.
function Get-TestFloor {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][int]$Default
    )
    $raw = [Environment]::GetEnvironmentVariable($Name)
    if ([string]::IsNullOrWhiteSpace($raw)) { return $Default }
    $parsed = 0
    if (-not [int]::TryParse($raw.Trim(), [ref]$parsed)) {
        Write-Fail "$Name must be an integer; got '$raw'"
        Exit-Soak 2
    }
    if ($parsed -lt $Default) {
        Write-Fail "$Name=$parsed would LOWER the built-in floor of $Default. Floors ratchet up only."
        Exit-Soak 2
    }
    return $parsed
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
        Exit-Soak 1
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
    # `minTests` is the POSITIVE LOWER BOUND on cases each file-level selector
    # must actually execute. `--no-tests=fail` only catches a selector that
    # matches NOTHING; it says nothing about a selector that used to match
    # thirteen cases and now matches one. Counts below are enumerated from the
    # sources themselves (both files are `#![cfg(windows)]`, and
    # `--run-ignored all` runs the ignored and non-ignored cases alike):
    #   live_fs_acl.rs                      12 #[ignore]d ACL tests
    #                                      +  1 native_acceptance_gate_marker = 13
    #   hard_process_containment_windows.rs  5 #[ignore]d Job-Object tests
    #                                      +  1 native_containment_gate_marker = 6
    # Adding a test keeps the gate green (the bound is `>=`, and the file-level
    # selector picks it up automatically); losing one turns it red, which is the
    # whole reason 20A-01 moved off hand-enumerated test names.
    $liveSuites = @(
        @{ id = 'live_fs_acl'; minTests = 13; args = @('-p', 'wcore-sandbox', '--test', 'live_fs_acl') },
        @{ id = 'hard_process_containment_windows'; minTests = 6; args = @('-p', 'wcore-sandbox', '--test', 'hard_process_containment_windows') }
    )

    $failed = @()
    foreach ($suite in $liveSuites) {
        $log = Join-Path $ResultsDir "L-$($suite.id).log"
        $floorVar = "WAYLAND_SOAK_MIN_TESTS_$($suite.id.ToUpperInvariant())"
        $floor = Get-TestFloor -Name $floorVar -Default $suite.minTests
        $nextestArgs = @('nextest', 'run', '--run-ignored', 'all', '--no-tests=fail', '--no-fail-fast') + $suite.args + @('--nocapture')
        # Read $LASTEXITCODE AFTER the pipeline, never as the last statement of a
        # `$x = & { … }` block: Tee-Object passes every line through, so such a
        # block returns an ARRAY of (all output lines + the exit code), and
        # `if ($array -ne 0)` is an array FILTER whose non-empty result is always
        # truthy. That made this phase report failure on a fully green run — as
        # measured in job 89752739969, where all 12 live_fs_acl and all 6
        # hard_process_containment_windows tests passed and PHASE L still failed.
        # The same idiom in PHASE F, combined with `exit <array>` collapsing to a
        # process exit code of 0, is what produced a week of zero-test greens.
        cargo @nextestArgs 2>&1 | Tee-Object -FilePath $log
        $suiteExit = $LASTEXITCODE
        if ($null -eq $suiteExit -or $suiteExit -is [array] -or $suiteExit -is [string]) {
            $t = if ($null -eq $suiteExit) { '<null>' } else { $suiteExit.GetType().FullName }
            Write-Fail "live-acceptance suite $($suite.id) produced a non-integer exit status of type $t (array-capture defect)"
            Exit-Soak 70
        }

        $executed = Get-NextestExecutedCount -LogPath $log
        Write-Note "suite $($suite.id): exit=$suiteExit executed=$executed floor=$floor"

        if ($suiteExit -ne 0) {
            Write-Fail "live-acceptance suite $($suite.id) failed with exit code $suiteExit"
            $failed += $suite.id
        }
        elseif ($executed -lt 0) {
            Write-Fail "live-acceptance suite $($suite.id): no nextest Summary line in $log — cannot prove a single test ran"
            $failed += "$($suite.id) (no summary line)"
        }
        elseif ($executed -lt $floor) {
            Write-Fail "live-acceptance suite $($suite.id) executed $executed tests, below the required floor of $floor"
            Write-Fail "A suite that runs too few tests is a FAILED suite, not a passing one."
            $failed += "$($suite.id) ($executed < $floor)"
        }
        else {
            Write-Ok "live-acceptance suite $($suite.id) passed ($executed tests, floor $floor)"
        }
    }

    if ($failed.Count -gt 0) {
        Write-Fail "PHASE L failed: $($failed -join ', ')"
        Exit-Soak 71
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
        Exit-Soak 1
    }
}
Write-Ok "toolchain present ($($RequiredTools -join ', '))"

# Print version info for diagnostics. These are native commands: `Get-Command`
# above proves only that a file exists on PATH, not that it runs — a
# half-installed cargo subcommand resolves fine and then exits nonzero. Check
# each one rather than letting a broken toolchain reach the gates below and
# fail there for the wrong reason.
$VersionLog = Join-Path $ResultsDir "0-versions.log"
& cargo --version 2>&1 | Tee-Object -FilePath $VersionLog | Out-Host
Assert-NativeExit -Code $LASTEXITCODE -What "cargo --version"
& rustc --version 2>&1 | Tee-Object -FilePath $VersionLog -Append | Out-Host
Assert-NativeExit -Code $LASTEXITCODE -What "rustc --version"
& cargo nextest --version 2>&1 | Tee-Object -FilePath $VersionLog -Append | Out-Host
Assert-NativeExit -Code $LASTEXITCODE -What "cargo nextest --version"
if ($LiveAcceptance -ne "only") {
    & cargo mutants --version 2>&1 | Tee-Object -FilePath $VersionLog -Append | Out-Host
    Assert-NativeExit -Code $LASTEXITCODE -What "cargo mutants --version"
}

# `only` mode runs the live-acceptance surface alone: it exists so the
# AppContainer-capable self-hosted runner can carry PHASE L nightly without
# re-running the whole windows-2022 workload it already covers.
if ($LiveAcceptance -eq "only") {
    Invoke-LiveAcceptancePhase -ResultsDir $ResultsDir
    Write-Phase "WINDOWS LIVE-ACCEPTANCE SOAK: PASS"
    Write-Host "Results saved to: $ResultsDir" -ForegroundColor Green
    Exit-Soak 0
}

# -----------------------------------------------------------------------------
# Phase F: cargo build --release -p wcore-cli
# -----------------------------------------------------------------------------
Write-Phase "PHASE F — cargo build --release -p wcore-cli"
$BuildLog = Join-Path $ResultsDir "F-build.log"
# Same exit-code capture rule as PHASE L below: read $LASTEXITCODE after the
# pipeline, not as the trailing value of a `& { … }` block (which returns the
# piped output as well, making every comparison an always-true array filter).
cargo build --release -p wcore-cli 2>&1 | Tee-Object -FilePath $BuildLog
$buildExit = $LASTEXITCODE
Assert-NativeExit -Code $buildExit -What "PHASE F release build"

# Verify binary exists. Windows MSVC target produces wayland-core.exe.
$BinaryCandidates = @(
    (Join-Path $RepoRoot "target\release\wayland-core.exe"),
    (Join-Path $RepoRoot "target\release\wayland-core")
)
$Binary = $BinaryCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $Binary) {
    Write-Fail "wayland-core binary not found at expected target/release/ location"
    Get-ChildItem (Join-Path $RepoRoot "target\release") | Select-Object -First 20 | Out-Host
    Exit-Soak 1
}
Write-Ok "binary at $Binary"

& $Binary --version 2>&1 | Tee-Object -FilePath (Join-Path $ResultsDir "F-binary.log") | Out-Host
Assert-NativeExit -Code $LASTEXITCODE -What "PHASE F wayland-core --version"

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
#
# `--no-tests=fail` is passed EXPLICITLY at the call site. It is a CLI option
# only — a `no-tests = "fail"` key under `[profile.default]` in
# .config/nextest.toml is silently ignored by cargo-nextest (measured; see the
# anti-vacuity note at the top of that file), so relying on it is relying on
# whatever default the installed nextest happens to ship.
#
# It is still not sufficient on its own: `--no-tests=fail` catches a selector
# that matches NOTHING, but says nothing about a run whose count silently
# collapses from hundreds to a handful because a crate stopped compiling its
# test targets under a cfg. Hence the executed-count floor below.
#
# `--no-fail-fast` (added 2026-08-29, FerroxLabs/wayland-core#350). Without it
# nextest CANCELS the run at the first failure, so PHASE G reported only the
# EARLIEST defect and everything ordered after it was invisible. Measured, not
# argued: run 33258858506 stopped at `3060/4123` on the core#374 hard failure;
# with that one defect fixed and nothing else changed, run 33266413002 reached
# `3883/4126` and surfaced three timeouts in `bash_unsaved_guard_bound_live`
# that had been hidden behind it for the whole cycle. A soak whose report is a
# lower bound cannot be used to decide that Windows is green, which is exactly
# what #350 c5 asks it to decide. The same reasoning is already recorded in
# ci.yml for the containerized Linux leg.
$NextestLog = Join-Path $ResultsDir "G-nextest.log"
cargo nextest run --no-tests=fail --no-fail-fast `
    -p wcore-cron `
    -p wcore-config `
    -p wcore-providers `
    -p wcore-tools `
    -p wcore-swarm `
    -p wcore-sandbox `
    2>&1 | Tee-Object -FilePath $NextestLog
$nextestExit = $LASTEXITCODE
Assert-NativeExit -Code $nextestExit -What "PHASE G nextest"

# Floor rationale. This is deliberately set WELL BELOW the true count so the
# gate cannot be permanently red, and enormously above the zero it was
# effectively enforcing. Two in-repo measurements bound it from below:
# .config/nextest.toml records "wcore-swarm ALONE … 150 tests run, 150 passed",
# and the PHASE G comment above records wcore-sandbox holding 105 of the
# workspace's 155 Windows-only tests — before wcore-cron, wcore-config,
# wcore-providers and wcore-tools contribute anything.
# RATCHET THIS UP to roughly the observed count once a real green run reports
# one: the printed "PHASE G executed N tests" line is the number to use.
# WAYLAND_SOAK_MIN_TESTS_G may raise it; by construction it can never lower it.
$MinTestsG = Get-TestFloor -Name "WAYLAND_SOAK_MIN_TESTS_G" -Default 100
Assert-TestsExecuted -Phase "PHASE G" -LogPath $NextestLog -Minimum $MinTestsG
Write-Ok "nextest run complete (6 crates)"

# -----------------------------------------------------------------------------
# Phase H: cargo mutants smoke (list-only)
# -----------------------------------------------------------------------------
Write-Phase "PHASE H — cargo mutants smoke (--list, no actual mutations)"
#
# This phase used to be pure theatre: `$LASTEXITCODE` was never read, the line
# count was computed and then never compared against anything, and
# `Write-Ok "mutants enumerator validated"` was unconditional. A crashed
# enumerator, or one listing zero mutants, "validated" exactly as loudly as a
# working one — and the line count it printed counted stderr noise as readily
# as mutants, because 2>&1 merges both into the log.
$MutantsLog = Join-Path $ResultsDir "H-mutants.log"
& cargo mutants --list -p wcore-providers 2>&1 | Tee-Object -FilePath $MutantsLog | Out-Host
Assert-NativeExit -Code $LASTEXITCODE -What "PHASE H cargo mutants --list"

# Floor deliberately conservative: wcore-providers implements four providers, so
# its real mutant population is in the hundreds. Anything under 20 means the
# enumerator did not actually enumerate. Ratchet up once observed.
$MinMutantsH = Get-TestFloor -Name "WAYLAND_SOAK_MIN_MUTANTS_H" -Default 20
$MutantCount = Get-MutantListCount -LogPath $MutantsLog
if ($MutantCount -lt 0) {
    Write-Fail "PHASE H — $MutantsLog is missing or empty; the enumerator produced no output at all"
    Exit-Soak 71
}
if ($MutantCount -lt $MinMutantsH) {
    Write-Fail "PHASE H enumerated $MutantCount mutants, below the required floor of $MinMutantsH."
    Write-Fail "AN ENUMERATOR THAT ENUMERATES NOTHING IS A FAILED GATE, not a validated one."
    Exit-Soak 71
}
Write-Ok "mutants enumerator listed $MutantCount mutants (floor $MinMutantsH)"

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
Exit-Soak 0
