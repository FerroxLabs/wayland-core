# Windows byte-range locking is MANDATORY, not advisory — measured directly.
#
# The single-owner lease (`wcore_cron::lease`) splits a one-byte `schedule.lock`
# sentinel from a freely readable `schedule.owner` record, on the stated grounds
# that `LockFileEx` excludes READERS as well as lock holders. That premise had
# never been measured on this program. This probe measures it, product-free, so
# the design rationale rests on an observation rather than on documentation.
#
# .NET's FileStream.Lock/Unlock map to LockFileEx/UnlockFileEx.
#
# Controls run in BOTH directions:
#   A. locked range   -> a read MUST fail   (can it fail)
#   B. unlocked range -> a read MUST succeed (can it pass)
#   C. after unlock   -> a read MUST succeed (the lock is not permanent)
# If B or C failed, the instrument would be dead and A would prove nothing.

$ErrorActionPreference = 'Continue'
$dir = $args[0]
if (-not $dir) { $dir = "D:\wls\out" }
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$probe  = Join-Path $dir "mandatory-probe.bin"
$report = Join-Path $dir "mandatory-lock-probe.txt"

$lines = @()
$lines += "PROBE=windows-mandatory-byte-range-lock"
$lines += "HOST=$env:COMPUTERNAME"
$lines += "UTC=$( (Get-Date).ToUniversalTime().ToString('o') )"

# 16 bytes: 0-7 will be locked, 8-15 left free as the in-run known-positive.
[System.IO.File]::WriteAllBytes($probe, [byte[]](0..15))

$holder = [System.IO.File]::Open($probe, 'Open', 'ReadWrite', 'ReadWrite')
$holder.Lock(0, 8)
$lines += "HOLDER_LOCKED_RANGE=0..7"

function Try-ReadRange([string]$path, [int]$offset, [int]$count) {
    try {
        $fs = [System.IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
        try {
            $fs.Seek($offset, 'Begin') | Out-Null
            $buf = New-Object byte[] $count
            $n = $fs.Read($buf, 0, $count)
            return "OK:$n"
        } finally { $fs.Close() }
    } catch {
        # Report the exception IDENTITY, not a masked HResult. `-band 0xFFFF` on a
        # .NET IOException recovers the CLR facility code (0x1501), not the Win32
        # code, so the first run of this probe reported a meaningless `5377`.
        # Repaired here rather than written up: a documented instrument defect is
        # a defect you have agreed to keep (LANE-BRIEF 6b-ii).
        $ex = $_.Exception
        $inner = $ex
        while ($inner.InnerException) { $inner = $inner.InnerException }
        $win32 = 'none'
        if ($inner -is [System.ComponentModel.Win32Exception]) {
            $win32 = $inner.NativeErrorCode
        }
        $msg = ($ex.Message -replace '\s+', ' ')
        return "ERR type=$($ex.GetType().Name) win32=$win32 msg=$msg"
    }
}

# A. the LOCKED range — the claim under test.
$lines += "A_READ_LOCKED_RANGE=$(Try-ReadRange $probe 0 8)"
# B. an UNLOCKED range of the SAME file, same run — proves the reader works.
$lines += "B_READ_UNLOCKED_RANGE=$(Try-ReadRange $probe 8 8)"

$holder.Unlock(0, 8)
# C. the same range after release — proves the exclusion is not permanent.
$lines += "C_READ_AFTER_UNLOCK=$(Try-ReadRange $probe 0 8)"
$holder.Close()

# D. a lock placed PAST end-of-file, which is what the journal writer lease does
#    (`session_journal/lease.rs` locks one byte at u64::MAX-1). Real bytes must
#    stay readable. .NET's FileStream.Lock takes an Int64, so u64::MAX-1 is not
#    expressible here; Int64::MaxValue-1 is the same side of EOF and makes the
#    same point. The Rust path is measured directly by the crate's own tests.
$pastEof = [int64]::MaxValue - 1
$holder2 = [System.IO.File]::Open($probe, 'Open', 'ReadWrite', 'ReadWrite')
$holder2.Lock($pastEof, 1)
$lines += "D_PAST_EOF_LOCK_OFFSET=$pastEof"
$lines += "D_READ_WHILE_PAST_EOF_LOCK_HELD=$(Try-ReadRange $probe 0 8)"
$holder2.Unlock($pastEof, 1)
$holder2.Close()

# E. cross-HANDLE exclusion inside ONE process — the property LockFileEx must
#    have for the in-process lease tests to mean anything.
$h1 = [System.IO.File]::Open($probe, 'Open', 'ReadWrite', 'ReadWrite')
$h1.Lock(0, 1)
$h2 = [System.IO.File]::Open($probe, 'Open', 'ReadWrite', 'ReadWrite')
try { $h2.Lock(0, 1); $lines += "E_SECOND_HANDLE_SAME_PROCESS=ACQUIRED"; $h2.Unlock(0,1) }
catch {
    $inner = $_.Exception; while ($inner.InnerException) { $inner = $inner.InnerException }
    $w = 'none'; if ($inner -is [System.ComponentModel.Win32Exception]) { $w = $inner.NativeErrorCode }
    $lines += "E_SECOND_HANDLE_SAME_PROCESS=REFUSED type=$($_.Exception.GetType().Name) win32=$w"
}
$h2.Close(); $h1.Unlock(0,1); $h1.Close()

# F. SELF-TEST of this probe's own reader. A dead reader returns ERR for
#    everything and would make A look like a pass for free. This reads an
#    UNLOCKED file with no holder at all: it must succeed. Three assertions
#    rather than two — known-positive, known-negative, and proof the reader is
#    not simply always-failing.
$clean = Join-Path $dir "mandatory-probe-clean.bin"
[System.IO.File]::WriteAllBytes($clean, [byte[]](0..7))
$lines += "F_SELFTEST_READ_UNLOCKED_FILE=$(Try-ReadRange $clean 0 8)"
$lines += "F_SELFTEST_READ_MISSING_FILE=$(Try-ReadRange (Join-Path $dir 'does-not-exist.bin') 0 8)"

$lines += "PROBEDONE"
Set-Content -Path $report -Value $lines
Get-Content $report
