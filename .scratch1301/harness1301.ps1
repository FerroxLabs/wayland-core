# Interleaved timing harness for wayland#1301 c3:
#   fix1_dispatch_budget_aborts_with_partial_result against its 240s kill line.
# Runs the EXACT CI isolated-step nextest invocation per arm, interleaved, and
# records every sample. Optional decomposition sampling (CPU user/kernel, IO
# ops, child processes) and a WerFault poll on EVERY run.
param(
    [Parameter(Mandatory = $true)][string]$Src,
    [Parameter(Mandatory = $true)][string]$Out,
    [int]$N = 12,
    [int]$SampledN = 0,
    [string[]]$Arms = @('release', 'debug'),
    [string]$CargoExe = 'cargo',
    [string[]]$CargoPrefix = @(),
    [int]$LoadThreads = 0,
    [switch]$CheckRunner,
    [int]$MaxIdleWaitMin = 90
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Out | Out-Null
$csv = Join-Path $Out 'samples.csv'
$header = 'phase,round,arm,start_iso,workers_before,cpu_load_before,idle_wait_s,workers_after,contaminated,wall_s,nextest_s,slow_line,result,exit_code,test_pid,user_cpu_s,kernel_cpu_s,threads_max,read_ops,write_ops,other_ops,write_bytes,children,werfault'
if (-not (Test-Path $csv)) { $header | Out-File -FilePath $csv -Encoding ascii }

function Get-Workers { @(Get-Process -Name 'Runner.Worker' -ErrorAction SilentlyContinue).Count }
function Get-Load { [int](Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average }

function Invoke-Arm([string]$phase, [int]$round, [string]$arm, [bool]$sampled) {
    $idleWait = 0
    $workersBefore = -1
    if ($CheckRunner) {
        $workersBefore = Get-Workers
        $sw0 = [Diagnostics.Stopwatch]::StartNew()
        while ($workersBefore -gt 0 -and $sw0.Elapsed.TotalMinutes -lt $MaxIdleWaitMin) {
            Start-Sleep -Seconds 30
            $workersBefore = Get-Workers
        }
        $idleWait = [math]::Round($sw0.Elapsed.TotalSeconds, 0)
    }
    $loadBefore = Get-Load
    $cargoArgs = @() + $CargoPrefix + @('nextest', 'run', '--locked')
    if ($arm -like 'release*') { $cargoArgs += '--release' }
    $cargoArgs += @('-p', 'wcore-agent', '--test', 'workflow_limits_test', '--profile', 'ci', '--retries', '0', '--no-tests=fail', '--test-threads', '1', '-E', 'test(=fix1_dispatch_budget_aborts_with_partial_result)')
    $tag = '{0}-r{1:D2}-{2}' -f $phase, $round, $arm
    $so = Join-Path $Out "$tag.out.txt"
    $se = Join-Path $Out "$tag.err.txt"

    $spinners = @()
    if ($arm -like '*loaded' -and $LoadThreads -gt 0) {
        for ($k = 0; $k -lt $LoadThreads; $k++) {
            $spinners += Start-Process -FilePath 'powershell' -ArgumentList @('-NoProfile', '-Command', 'while($true){}') -PassThru -WindowStyle Hidden
        }
        Start-Sleep -Seconds 2
    }

    $start = Get-Date -Format o
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $CargoExe -ArgumentList $cargoArgs -WorkingDirectory $Src -RedirectStandardOutput $so -RedirectStandardError $se -PassThru -NoNewWindow
    $wer = @{}
    $testPid = 0; $u = 0.0; $kern = 0.0; $thr = 0
    $rops = 0; $wops = 0; $oops = 0; $wbytes = 0
    $children = @{}
    $tick = 0
    while (-not $p.HasExited) {
        Start-Sleep -Milliseconds 250
        $tick++
        if ($tick % 4 -eq 0) {
            foreach ($w in @(Get-Process -Name 'WerFault', 'WerFaultSecure' -ErrorAction SilentlyContinue)) {
                if (-not $wer.ContainsKey($w.Id)) {
                    $cl = (Get-CimInstance Win32_Process -Filter "ProcessId=$($w.Id)" -ErrorAction SilentlyContinue).CommandLine
                    $wer[$w.Id] = ($w.Name + ':' + $cl) -replace '[,\r\n]', ' '
                }
            }
        }
        if ($sampled) {
            $t = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'workflow_limits_test-*' })
            if ($t.Count -gt 0) {
                $tp = $t[0]
                try {
                    $testPid = $tp.Id
                    $u = [math]::Max($u, $tp.UserProcessorTime.TotalSeconds)
                    $kern = [math]::Max($kern, $tp.PrivilegedProcessorTime.TotalSeconds)
                    $thr = [math]::Max($thr, $tp.Threads.Count)
                } catch {}
                if ($tick % 4 -eq 0) {
                    $ci = Get-CimInstance Win32_Process -Filter "ProcessId=$testPid" -ErrorAction SilentlyContinue
                    if ($ci) {
                        $rops = [math]::Max($rops, [double]$ci.ReadOperationCount)
                        $wops = [math]::Max($wops, [double]$ci.WriteOperationCount)
                        $oops = [math]::Max($oops, [double]$ci.OtherOperationCount)
                        $wbytes = [math]::Max($wbytes, [double]$ci.WriteTransferCount)
                    }
                    foreach ($c in @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$testPid" -ErrorAction SilentlyContinue)) {
                        $children[$c.ProcessId] = $c.Name
                    }
                }
            }
        }
    }
    $p.WaitForExit()
    $wall = [math]::Round($sw.Elapsed.TotalSeconds, 3)
    $exit = $p.ExitCode
    foreach ($s in $spinners) { try { Stop-Process -Id $s.Id -Force -ErrorAction SilentlyContinue } catch {} }

    $workersAfter = -1; $contam = 0
    if ($CheckRunner) {
        $workersAfter = Get-Workers
        if ($workersBefore -gt 0 -or $workersAfter -gt 0) { $contam = 1 }
    }
    $text = ((Get-Content $so -Raw -ErrorAction SilentlyContinue) + "`n" + (Get-Content $se -Raw -ErrorAction SilentlyContinue))
    $ns = ''
    $m = [regex]::Match($text, 'PASS \[\s*([0-9.]+)s\]\s*(\(\d+/\d+\)\s*)?wcore-agent::workflow_limits_test fix1_dispatch_budget_aborts_with_partial_result')
    $result = 'UNPARSED'
    if ($m.Success) { $ns = $m.Groups[1].Value; $result = 'PASS' }
    else {
        $mf = [regex]::Match($text, '(FAIL|TIMEOUT|SIGABRT|ABORT)\s*\[\s*([0-9.]+)s\]')
        if ($mf.Success) { $result = $mf.Groups[1].Value; $ns = $mf.Groups[2].Value }
    }
    $slow = ([regex]::Matches($text, 'SLOW \[>\s*[0-9.]+s\]') | ForEach-Object { $_.Value }) -join ' '
    $childStr = ($children.GetEnumerator() | ForEach-Object { "$($_.Value)#$($_.Key)" }) -join ' '
    $werStr = ($wer.Values) -join ' | '
    $row = @($phase, $round, $arm, $start, $workersBefore, $loadBefore, $idleWait, $workersAfter, $contam, $wall, $ns, $slow, $result, $exit, $testPid, [math]::Round($u, 3), [math]::Round($kern, 3), $thr, $rops, $wops, $oops, $wbytes, $childStr, $werStr) -join ','
    $row | Out-File -FilePath $csv -Append -Encoding ascii
    Write-Output $row
}

"HARNESS_START $(Get-Date -Format o) N=$N SampledN=$SampledN Arms=$($Arms -join '/') COMPUTERNAME=$env:COMPUTERNAME CORES=$([Environment]::ProcessorCount)" | Tee-Object -FilePath (Join-Path $Out 'meta.txt') -Append

# Headroom phase: plain runs (WerFault poll only), arm order rotated each round.
for ($r = 1; $r -le $N; $r++) {
    $shift = ($r - 1) % $Arms.Count
    $order = @($Arms[$shift..($Arms.Count - 1)]) + @(if ($shift -gt 0) { $Arms[0..($shift - 1)] })
    foreach ($a in $order) { Invoke-Arm 'plain' $r $a $false }
}
# Decomposition phase: sampled runs, interleaved, reported separately.
for ($r = 1; $r -le $SampledN; $r++) {
    $shift = ($r - 1) % $Arms.Count
    $order = @($Arms[$shift..($Arms.Count - 1)]) + @(if ($shift -gt 0) { $Arms[0..($shift - 1)] })
    foreach ($a in $order) { Invoke-Arm 'sampled' $r $a $true }
}
"HARNESS_DONE $(Get-Date -Format o)" | Tee-Object -FilePath (Join-Path $Out 'meta.txt') -Append
