[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{40}([0-9a-f]{24})?$')]
    [string]$ExpectedCommit,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{40}([0-9a-f]{24})?$')]
    [string]$ExpectedTree,

    # Candidate-bound request nonce. Propagated verbatim into every target and
    # final acceptance marker so f20-native-uat-proof.mjs can bind this job's
    # log to the exact dispatch request. Strict lowercase hex, 32-64 chars.
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{32,64}$')]
    [string]$Nonce
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Git([string[]]$Arguments) {
    $output = & git @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
    return ($output | Out-String).Trim()
}

if ($env:WAYLAND_F20_NATIVE_ACCEPTANCE -ne '1') {
    throw 'native F20 acceptance requires WAYLAND_F20_NATIVE_ACCEPTANCE=1'
}

$scriptRoot = Split-Path -Parent $PSScriptRoot
$repositoryRoot = Invoke-Git @('rev-parse', '--show-toplevel')
$resolvedScriptRoot = (Resolve-Path -LiteralPath $scriptRoot).Path
$resolvedRepositoryRoot = (Resolve-Path -LiteralPath $repositoryRoot).Path
if ($resolvedRepositoryRoot -ne $resolvedScriptRoot) {
    throw "wrong repository: expected $resolvedScriptRoot, observed $resolvedRepositoryRoot"
}
if (-not (Test-Path -LiteralPath (Join-Path $repositoryRoot 'crates/wcore-sandbox/Cargo.toml'))) {
    throw 'wrong repository: wcore-sandbox manifest is absent'
}

$status = Invoke-Git @('status', '--porcelain=v1', '--untracked-files=all')
if ($status.Length -ne 0) {
    throw "native F20 acceptance requires a clean checkout`n$status"
}

$expectedCommit = $ExpectedCommit.ToLowerInvariant()
$expectedTree = $ExpectedTree.ToLowerInvariant()
$actualCommit = Invoke-Git @('rev-parse', 'HEAD')
$actualTree = Invoke-Git @('rev-parse', 'HEAD^{tree}')
if ($actualCommit -ne $expectedCommit) {
    throw "wrong commit: expected $expectedCommit, observed $actualCommit"
}
if ($actualTree -ne $expectedTree) {
    throw "wrong tree: expected $expectedTree, observed $actualTree"
}

$env:WAYLAND_SANDBOX_LIVE_WINDOWS = '1'

# Each exact native-acceptance target: an id (mirrored in
# f20-native-uat-proof.mjs WINDOWS_TARGETS, same order), the OS it proves, and
# the exact ignored-only nextest selector that proves it. `--run-ignored all`
# runs the native-acceptance tests that are #[ignore]d off the real host;
# `--no-tests=fail` fails closed if a selector matches nothing (a renamed/absent
# test cannot silently pass).
#
# The two containment targets select the REAL Windows Job-Object tests in
# crates/wcore-sandbox/tests/hard_process_containment_windows.rs (REQ-native-r7).
# They were previously wired to crates/wcore-sandbox/tests/hard_process_containment.rs,
# which is entirely Bubblewrap/Linux-only and can NEVER pass on Windows (BRIEF §4.F).
#
# The `os` field drives the wrong-OS anti-drift guard below (REQ-native-r8):
#   * 'windows' — an OS-specific native test whose source MUST be cfg-gated for
#     Windows; the guard fails closed if it is not (or is gated for another OS).
#   * 'any'     — a legitimately cross-platform test (no OS-exclusive gate; runs
#     on Windows AND Linux, e.g. dispatch_smoke, the f20 lifecycle) — exempt.
# The id -> { crate, test, os } expectation mirrors WINDOWS_TARGET_SOURCES in
# f20-native-uat-proof.mjs, the shared canonical map the macOS guard reuses (20-22).
$targets = @(
    @{ id = 'windows-retained-handle';           os = 'windows'; args = @('-p', 'wcore-sandbox', '--test', 'live_fs_acl', '-E', 'test(one_execution_grant_never_leaks_to_another_identity)') },
    @{ id = 'windows-appcontainer-acl';           os = 'windows'; args = @('-p', 'wcore-sandbox', '--test', 'live_fs_acl', '-E', 'test(granted_path_is_readable_then_revoked)') },
    @{ id = 'windows-job-object';                 os = 'windows'; args = @('-p', 'wcore-sandbox', '--test', 'hard_process_containment_windows', '-E', 'test(/contained_detached_child_exit|job_close_reaps_detached_descendant_with_no_residue|active_process_cap_is_enforced|breakaway_is_denied/)') },
    @{ id = 'windows-public-dispatch';            os = 'any';     args = @('-p', 'wcore-swarm', '--test', 'dispatch_smoke') },
    @{ id = 'windows-hard-process-containment';   os = 'windows'; args = @('-p', 'wcore-sandbox', '--test', 'hard_process_containment_windows', '-E', 'test(qualified_hard_containment_backend_preflight)') },
    @{ id = 'windows-f20-lifecycle';              os = 'any';     args = @('-p', 'wcore-agent', '--test', 'transactional_delegated_mutation_test') }
)

# ---- Wrong-OS anti-drift guard (REQ-native-r8 / BRIEF §4.F/§4.G) -------------
#
# The systemic defect this closes: a native proof target silently mapped to a
# test gated for a DIFFERENT OS (the two Windows containment targets pointed at
# the Linux-only Bubblewrap test), so the target "passed" or "skipped" without
# ever proving the platform property. This guard makes that impossible: before a
# target runs, an OS-specific target's selected test source MUST be affirmatively
# cfg-gated for the target's own OS, and MUST NOT be gated for a foreign OS —
# otherwise the proof fails closed. Cross-platform ('any') targets are exempt.

function Get-TargetTestSource {
    param([hashtable]$Target, [string]$RepositoryRoot)
    $crate = $null
    $test = $null
    for ($i = 0; $i -lt $Target.args.Count; $i++) {
        if ($Target.args[$i] -eq '-p') { $crate = $Target.args[$i + 1] }
        elseif ($Target.args[$i] -eq '--test') { $test = $Target.args[$i + 1] }
    }
    if (-not $crate -or -not $test) { return $null }
    return (Join-Path $RepositoryRoot (Join-Path 'crates' (Join-Path $crate (Join-Path 'tests' "$test.rs"))))
}

function Assert-TargetOsGate {
    param([hashtable]$Target, [string]$RepositoryRoot)

    if (-not $Target.ContainsKey('os')) {
        throw "anti-drift: target $($Target.id) has no declared os"
    }
    $expectedOs = $Target.os
    # A cross-platform target legitimately runs on Windows with no OS-exclusive
    # gate; nothing to assert (it also runs, and is proven, on the Linux leg).
    if ($expectedOs -eq 'any') { return }

    $source = Get-TargetTestSource -Target $Target -RepositoryRoot $RepositoryRoot
    if (-not $source) {
        throw "anti-drift: target $($Target.id) declares os=$expectedOs but no --test source could be derived from its args"
    }
    if (-not (Test-Path -LiteralPath $source)) {
        throw "anti-drift: target $($Target.id) (os=$expectedOs) selects a missing test source: $source"
    }
    $text = Get-Content -LiteralPath $source -Raw

    # Positive gate: the selected source must be cfg-gated for the target OS. This
    # is the load-bearing check — the mis-wired Bubblewrap test carries NO Windows
    # cfg, so a Windows target pointed back at it fails closed here.
    $positive = $false
    if ($expectedOs -eq 'windows') {
        $positive = ($text -match 'cfg\(\s*windows\s*\)') -or ($text -match 'cfg\(\s*target_os\s*=\s*"windows"\s*\)')
    }
    elseif ($expectedOs -eq 'macos') {
        $positive = ($text -match 'cfg\(\s*target_os\s*=\s*"macos"\s*\)')
    }
    else {
        throw "anti-drift: target $($Target.id) declares an unknown os=$expectedOs"
    }
    if (-not $positive) {
        throw "anti-drift: target $($Target.id) declares os=$expectedOs but its selected test source is not cfg-gated for $expectedOs (a wrong-OS or ungated test cannot prove $expectedOs containment): $source"
    }

    # Negative gate: a source affirmatively cfg-gated to a DIFFERENT OS must never
    # back this target (defense in depth beyond the positive check).
    $foreign = @('linux', 'macos', 'android', 'ios', 'freebsd', 'netbsd', 'openbsd', 'dragonfly') | Where-Object { $_ -ne $expectedOs }
    foreach ($other in $foreign) {
        $pattern = 'cfg\(\s*target_os\s*=\s*"' + [regex]::Escape($other) + '"\s*\)'
        if ($text -match $pattern) {
            throw "anti-drift: target $($Target.id) (os=$expectedOs) selects a test source cfg-gated for ${other}: $source"
        }
    }
    if ($expectedOs -eq 'windows' -and ($text -match 'cfg\(\s*unix\s*\)')) {
        throw "anti-drift: windows target $($Target.id) selects a unix-gated test source: $source"
    }
}

foreach ($target in $targets) {
    # Fail closed BEFORE running cargo if this target maps to a wrong-OS test.
    Assert-TargetOsGate -Target $target -RepositoryRoot $repositoryRoot

    $nextestArgs = @('nextest', 'run', '--run-ignored', 'all', '--no-tests=fail') + $target.args + @('--nocapture')
    & cargo @nextestArgs
    if ($LASTEXITCODE -ne 0) {
        throw "native Windows target $($target.id) failed with exit code $LASTEXITCODE"
    }
    Write-Host "F20_NATIVE_TARGET=PASS platform=windows target=$($target.id) commit=$expectedCommit tree=$expectedTree nonce=$Nonce"
}

# Exactly one final platform acceptance marker, emitted only after every
# target marker above. f20-native-uat-proof.mjs fails closed on any missing,
# duplicated, reordered, or pre-target marker.
Write-Host "F20_NATIVE_WINDOWS_ACCEPTANCE=PASS commit=$expectedCommit tree=$expectedTree nonce=$Nonce"
