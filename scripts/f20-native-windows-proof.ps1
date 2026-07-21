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
# f20-native-uat-proof.mjs WINDOWS_TARGETS, same order) and the exact
# ignored-only nextest selector that proves it. `--run-ignored all` runs the
# native-acceptance tests that are #[ignore]d off the real host; `--no-tests=fail`
# fails closed if a selector matches nothing (a renamed/absent test cannot
# silently pass).
$targets = @(
    @{ id = 'windows-retained-handle';           args = @('-p', 'wcore-sandbox', '--test', 'live_fs_acl', '-E', 'test(one_execution_grant_never_leaks_to_another_identity)') },
    @{ id = 'windows-appcontainer-acl';           args = @('-p', 'wcore-sandbox', '--test', 'live_fs_acl', '-E', 'test(granted_path_is_readable_then_revoked)') },
    @{ id = 'windows-job-object';                 args = @('-p', 'wcore-sandbox', '--test', 'hard_process_containment', '-E', 'test(contained_detached_child_exit)') },
    @{ id = 'windows-public-dispatch';            args = @('-p', 'wcore-swarm', '--test', 'dispatch_smoke') },
    @{ id = 'windows-hard-process-containment';   args = @('-p', 'wcore-sandbox', '--test', 'hard_process_containment', '-E', 'test(qualified_hard_containment_backend_preflight)') },
    @{ id = 'windows-f20-lifecycle';              args = @('-p', 'wcore-agent', '--test', 'transactional_delegated_mutation_test') }
)

foreach ($target in $targets) {
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
