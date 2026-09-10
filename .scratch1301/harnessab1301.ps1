# Base-vs-fix interleaved timing harness for wayland#1301 c3.
# Each arm is its own source tree with its own CARGO_TARGET_DIR. Every run is the
# EXACT CI isolated-step nextest invocation (release, --profile ci, --retries 0,
# --test-threads 1). Raw nextest output is kept per run as
#   plain-rNN-<arm>.err.txt / .out.txt
# so reparse1301.py grades it from the raw text, not from this script's CSV.
param(
    [Parameter(Mandatory = $true)][string]$Out,
    # "name|source-dir|target-dir", e.g. "base|D:\a\w|D:\a\w\target-base"
    [Parameter(Mandatory = $true)][string[]]$ArmSpecs,
    [int]$N = 6,
    [string]$CargoExe = 'cargo',
    [string[]]$CargoPrefix = @(),
    [switch]$CheckRunner,
    [int]$MaxIdleWaitMin = 90
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Out | Out-Null
$csv = Join-Path $Out 'samples.csv'
if (-not (Test-Path $csv)) {
    'round,arm,start_iso,workers_before,workers_after,wall_s,exit_code,werfault' | Out-File -FilePath $csv -Encoding ascii
}

$arms = @()
foreach ($spec in $ArmSpecs) {
    $parts = $spec.Split('|')
    if ($parts.Count -ne 3) { throw "bad arm spec (want name|source|target): $spec" }
    if (-not (Test-Path (Join-Path $parts[1] 'Cargo.toml'))) { throw "arm $($parts[0]) source has no Cargo.toml: $($parts[1])" }
    $arms += [pscustomobject]@{ Name = $parts[0]; Source = $parts[1]; Target = $parts[2] }
}

function Get-Workers { @(Get-Process -Name 'Runner.Worker' -ErrorAction SilentlyContinue).Count }

function Invoke-Arm([int]$round, $arm) {
    $workersBefore = -1
    if ($CheckRunner) {
        $workersBefore = Get-Workers
        $idle = [Diagnostics.Stopwatch]::StartNew()
        while ($workersBefore -gt 0 -and $idle.Elapsed.TotalMinutes -lt $MaxIdleWaitMin) {
            Start-Sleep -Seconds 30
            $workersBefore = Get-Workers
        }
    }
    $cargoArgs = @() + $CargoPrefix + @('nextest', 'run', '--locked', '--release', '-p', 'wcore-agent', '--test', 'workflow_limits_test', '--profile', 'ci', '--retries', '0', '--no-tests=fail', '--test-threads', '1', '-E', 'test(=fix1_dispatch_budget_aborts_with_partial_result)')
    $tag = 'plain-r{0:D2}-{1}' -f $round, $arm.Name
    $so = Join-Path $Out "$tag.out.txt"
    $se = Join-Path $Out "$tag.err.txt"
    $env:CARGO_TARGET_DIR = $arm.Target
    $start = Get-Date -Format o
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $CargoExe -ArgumentList $cargoArgs -WorkingDirectory $arm.Source -RedirectStandardOutput $so -RedirectStandardError $se -PassThru -NoNewWindow
    # Touching Handle before exit is what makes ExitCode populate afterwards.
    $null = $p.Handle
    $wer = @{}
    while (-not $p.HasExited) {
        Start-Sleep -Seconds 1
        foreach ($w in @(Get-Process -Name 'WerFault', 'WerFaultSecure' -ErrorAction SilentlyContinue)) { $wer[$w.Id] = $w.Name }
    }
    $p.WaitForExit()
    $wall = [math]::Round($sw.Elapsed.TotalSeconds, 3)
    $workersAfter = -1
    if ($CheckRunner) { $workersAfter = Get-Workers }
    $row = "$round,$($arm.Name),$start,$workersBefore,$workersAfter,$wall,$($p.ExitCode),$($wer.Count)"
    $row | Out-File -FilePath $csv -Append -Encoding ascii
    Write-Output $row
}

"HARNESS_AB_START $(Get-Date -Format o) N=$N arms=$(($arms | ForEach-Object { $_.Name }) -join '/') COMPUTERNAME=$env:COMPUTERNAME CORES=$([Environment]::ProcessorCount)" | Tee-Object -FilePath (Join-Path $Out 'meta.txt') -Append
for ($r = 1; $r -le $N; $r++) {
    $shift = ($r - 1) % $arms.Count
    $order = @($arms[$shift..($arms.Count - 1)]) + @(if ($shift -gt 0) { $arms[0..($shift - 1)] })
    foreach ($a in $order) { Invoke-Arm $r $a }
}
"HARNESS_AB_DONE $(Get-Date -Format o)" | Tee-Object -FilePath (Join-Path $Out 'meta.txt') -Append
