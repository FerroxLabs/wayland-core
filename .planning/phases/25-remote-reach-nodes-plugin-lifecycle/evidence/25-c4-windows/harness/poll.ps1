# Repaired build poller for lane 25-c4-windows.
#
# DEFECT IT REPAIRS (measured 2026-07-29, this lane): the first poller graded
# "build-status.txt absent" as STILL-BUILDING. But an absent status file is ALSO
# what a build that died produces - and mine had died, killed when the launching
# ssh session tore down its Start-Process child. The poller reported
# STILL-BUILDING for 12 consecutive iterations over ~9 minutes with no build
# running at all. That is a known-negative assertion self-passing on a dead
# instrument (LANE-BRIEF 3b-i).
#
# REPAIR: never infer "running" from an absence. Require a POSITIVE liveness
# signal - a cargo/rustc process whose command line belongs to THIS lane's tree -
# and grade three states, not two.
#
# Usage:  poll.ps1 <marker>     e.g. poll.ps1 lane-25c4-win
param([string]$Marker = "lane-25c4-win")

$status = "D:\lane-25c4-win\build-status.txt"

if (Test-Path $status) {
    $body = Get-Content $status
    if ($body -contains "WLDONE") { "STATE=DONE"; $body | ForEach-Object { "  $_" } }
    else { "STATE=UNREADABLE (status file present, WLDONE absent)" }
    exit
}

$live = @(Get-CimInstance Win32_Process | Where-Object {
    ($_.Name -eq "cargo.exe" -or $_.Name -eq "rustc.exe") -and $_.CommandLine -and $_.CommandLine.Contains($Marker)
})

if ($live.Count -gt 0) {
    "STATE=BUILDING liveness_procs=$($live.Count)"
} else {
    "STATE=DEAD no status file AND no cargo/rustc for marker '$Marker' - the build is NOT running"
}
