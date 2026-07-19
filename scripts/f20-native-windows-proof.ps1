[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-fA-F]{40}([0-9a-fA-F]{24})?$')]
    [string]$ExpectedCommit,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-fA-F]{40}([0-9a-fA-F]{24})?$')]
    [string]$ExpectedTree
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

$actualCommit = Invoke-Git @('rev-parse', 'HEAD')
$actualTree = Invoke-Git @('rev-parse', 'HEAD^{tree}')
if ($actualCommit -ne $ExpectedCommit.ToLowerInvariant()) {
    throw "wrong commit: expected $ExpectedCommit, observed $actualCommit"
}
if ($actualTree -ne $ExpectedTree.ToLowerInvariant()) {
    throw "wrong tree: expected $ExpectedTree, observed $actualTree"
}

$env:WAYLAND_SANDBOX_LIVE_WINDOWS = '1'
& cargo test -p wcore-sandbox --test live_fs_acl -- --ignored --nocapture
$testExit = $LASTEXITCODE
if ($testExit -ne 0) {
    throw "native F20 acceptance failed with exit code $testExit"
}

Write-Host "F20 native Windows acceptance PASS commit=$actualCommit tree=$actualTree"
