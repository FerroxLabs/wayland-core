# F26-03-D, the Windows half. Pure ASCII by rule: PowerShell 5.1 reads a
# BOM-less script as ANSI, and a UTF-8 em-dash then decodes to a smart quote
# which PowerShell accepts as a string delimiter -- a parse error that exits 1
# and reads like a passing self-red check (recorded as a defect in 26-03).
#
# Two legs:
#
#   1. DEEP. Create a tree whose restored paths run well past MAX_PATH, then
#      restore it. This is the regression proof for F26-03-D: before the fix
#      this exact leg failed with "os error 3" AFTER create had accepted the
#      tree, which is what made the archive unrestorable on the machine that
#      produced it.
#
#   2. IMPOSSIBLE NAMES. Restore an archive BUILT ON LINUX carrying names that
#      are ordinary there and cannot exist here (a reserved DOS device name and
#      a forbidden character). This must be REFUSED, must name both offenders,
#      and must leave the target unwritten. The refusal is measured by digesting
#      the target before and after -- not read off the message, which is how a
#      warn-and-continue would be caught.
#
# Usage: portability-longpath-proof.ps1 -Binary <exe> -Work <dir> -HostileArchive <file>

param(
    [Parameter(Mandatory=$true)][string]$Binary,
    [Parameter(Mandatory=$true)][string]$Work,
    [string]$HostileArchive = ''
)

$ErrorActionPreference = 'Continue'

# A gate whose binary is absent must go RED. `powershell -File missing.ps1;
# exit $LASTEXITCODE` returns 0, so absence must be asserted, never assumed.
if (-not (Test-Path -LiteralPath $Binary)) {
    Write-Output "PROOF-FAIL: binary not found: $Binary"
    exit 44
}
Write-Output "LONGPATH-PLATFORM: windows"
Write-Output ("BINARY: " + $Binary)
Write-Output ("OS-BUILD: " + [System.Environment]::OSVersion.Version.ToString())
$lpe = (Get-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' -Name LongPathsEnabled -ErrorAction SilentlyContinue).LongPathsEnabled
Write-Output ("LONGPATHSENABLED: " + $(if ($null -eq $lpe) { 'unset' } else { $lpe }))

if (Test-Path -LiteralPath $Work) { Remove-Item -LiteralPath $Work -Recurse -Force }
New-Item -ItemType Directory -Path $Work -Force | Out-Null

# ---------------- leg 1: the deep tree ----------------------------------------
$Src = Join-Path $Work 'src'
New-Item -ItemType Directory -Path $Src -Force | Out-Null

$segments = 1..8 | ForEach-Object { "deeply-nested-directory-segment-$_-padding" }
$DeepRel  = 'skills/' + ($segments -join '/')
$DeepDir  = Join-Path $Src ($DeepRel -replace '/', '\')
New-Item -ItemType Directory -Path $DeepDir -Force | Out-Null
Set-Content -LiteralPath (Join-Path $DeepDir 'deep-canary.md') -Value 'CANARY-DEEP-PAYLOAD' -NoNewline
Set-Content -LiteralPath (Join-Path $Src 'config.toml') -Value "[storage.credentials]`nbackend = `"plaintext`"`n" -NoNewline
New-Item -ItemType Directory -Path (Join-Path $Src 'memory') -Force | Out-Null
Set-Content -LiteralPath (Join-Path $Src 'memory\notes.md') -Value 'CANARY-MEMORY' -NoNewline
Write-Output ("DEEP-REL-LEN: " + $DeepRel.Length)

$DeepArchive = Join-Path $Work 'deep.tar.gz'
& $Binary backup create --home $Src --out $DeepArchive 2>&1 | Out-Null
$deepCreateExit = $LASTEXITCODE
Write-Output ("DEEP-CREATE-EXIT: " + $deepCreateExit)
if ($deepCreateExit -ne 0) { Write-Output "PROOF-FAIL: deep create failed"; exit 2 }

$srcDigest = (& $Binary backup digest --home $Src | Select-String -Pattern '^DIGEST: ').Line -replace '^DIGEST: ', ''
$Target = Join-Path $Work 'target'
& $Binary backup restore $DeepArchive --home $Target 2>&1 | Out-Null
$deepRestoreExit = $LASTEXITCODE
Write-Output ("DEEP-RESTORE-EXIT: " + $deepRestoreExit)
if ($deepRestoreExit -ne 0) {
    Write-Output "PROOF-FAIL: deep restore failed -- F26-03-D is NOT fixed"
    & $Binary backup restore $DeepArchive --home ($Target + '-diag') 2>&1 | Select-Object -Last 5
    exit 2
}
$targetDigest = (& $Binary backup digest --home $Target | Select-String -Pattern '^DIGEST: ').Line -replace '^DIGEST: ', ''
Write-Output ("DEEP-SRC-DIGEST:    " + $srcDigest)
Write-Output ("DEEP-TARGET-DIGEST: " + $targetDigest)
if ($srcDigest -ne $targetDigest) { Write-Output "PROOF-FAIL: deep tree did not round-trip byte-identically"; exit 2 }

$deepAbs = Join-Path $Target (($DeepRel + '/deep-canary.md') -replace '/', '\')
Write-Output ("DEEP-RESTORED-ABS-LEN: " + $deepAbs.Length)
if ($deepAbs.Length -le 260) { Write-Output "PROOF-FAIL: restored path is only $($deepAbs.Length) chars -- too shallow to have reached the defect"; exit 2 }
if (-not (Test-Path -LiteralPath $deepAbs)) { Write-Output "PROOF-FAIL: deep canary absent after restore"; exit 2 }
Write-Output "DEEP-CANARY-PRESENT: yes"

# ---------------- leg 2: names this platform cannot represent -----------------
if ($HostileArchive -eq '') {
    Write-Output "HOSTILE-LEG: NOT RUN -- no archive supplied"
    Write-Output "PROOF-PARTIAL: deep leg only"
    exit 0
}
if (-not (Test-Path -LiteralPath $HostileArchive)) {
    Write-Output ("PROOF-FAIL: hostile archive not found: " + $HostileArchive)
    exit 45
}

# The target carries state, so a refusal that wrote anything would be visible.
$HostileTarget = Join-Path $Work 'hostile-target'
New-Item -ItemType Directory -Path $HostileTarget -Force | Out-Null
Set-Content -LiteralPath (Join-Path $HostileTarget 'config.toml') -Value 'LIVE-PROFILE' -NoNewline
New-Item -ItemType Directory -Path (Join-Path $HostileTarget 'legacy') -Force | Out-Null
Set-Content -LiteralPath (Join-Path $HostileTarget 'legacy\keep.txt') -Value 'MUST-SURVIVE' -NoNewline
$preDigest = (& $Binary backup digest --home $HostileTarget | Select-String -Pattern '^DIGEST: ').Line -replace '^DIGEST: ', ''

$hostileOut = (& $Binary backup restore $HostileArchive --home $HostileTarget --replace 2>&1 | Out-String)
$hostileExit = $LASTEXITCODE
Write-Output ("HOSTILE-RESTORE-EXIT: " + $hostileExit)
$hostileOut -split "`n" | ForEach-Object { if ($_.Trim() -ne '') { Write-Output ("HOSTILE| " + $_.Trim()) } }

$postDigest = (& $Binary backup digest --home $HostileTarget | Select-String -Pattern '^DIGEST: ').Line -replace '^DIGEST: ', ''
Write-Output ("HOSTILE-PRE-DIGEST:  " + $preDigest)
Write-Output ("HOSTILE-POST-DIGEST: " + $postDigest)

$namesAux   = $hostileOut -match 'aux\.txt'
$namesColon = $hostileOut -match 'report:final\.md'
Write-Output ("HOSTILE-NAMES-AUX: " + $namesAux)
Write-Output ("HOSTILE-NAMES-COLON: " + $namesColon)

if ($hostileExit -eq 0) { Write-Output "PROOF-FAIL: an archive carrying names this platform cannot represent was NOT refused"; exit 3 }
if (-not $namesAux)   { Write-Output "PROOF-FAIL: refusal did not name aux.txt"; exit 3 }
if (-not $namesColon) { Write-Output "PROOF-FAIL: refusal did not name report:final.md"; exit 3 }
if ($preDigest -ne $postDigest) { Write-Output "PROOF-FAIL: a refusal wrote to the target"; exit 3 }
if ((Get-Content -LiteralPath (Join-Path $HostileTarget 'config.toml') -Raw) -ne 'LIVE-PROFILE') { Write-Output "PROOF-FAIL: live profile was modified by a refusal"; exit 3 }
if (-not (Test-Path -LiteralPath (Join-Path $HostileTarget 'legacy\keep.txt'))) { Write-Output "PROOF-FAIL: a file the archive does not contain was removed by a refusal"; exit 3 }

Write-Output "PROOF-OK: a deep tree round-trips exactly past MAX_PATH, and an archive carrying names this platform cannot represent is refused by name with the target left byte-identical"
exit 0
