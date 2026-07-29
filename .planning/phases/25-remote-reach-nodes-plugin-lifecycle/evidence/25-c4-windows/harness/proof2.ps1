# lane 25-c4-windows - FINAL consolidated proof, at the fix commit.
# UTF-8-faithful capture (see NOTES: PowerShell decodes native stdout with the OEM
# code page by default, which mangled the product's own text in the first captures).
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$OutputEncoding           = [System.Text.Encoding]::UTF8
$ErrorActionPreference    = "Continue"

$EV     = "D:\lane-25c4-ev"
$FIXED  = "D:\lane-25c4-win\target\debug\wayland-core.exe"
$BASE   = "C:\ferrox-25h\wayland-core.exe"          # pre-fix 0.12.25, read-only
$status = "$EV\proof-status.txt"
$utf8   = New-Object System.Text.UTF8Encoding($false)

Remove-Item $status -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path "$EV\cfg\allow" | Out-Null
New-Item -ItemType Directory -Force -Path "$EV\cfg\deny"  | Out-Null
Set-Location $EV

[IO.File]::WriteAllText("$EV\cfg\allow\config.toml", "[security]`nenabled = true`negress_allow = [`"api.machines.dev`"]`n", $utf8)
[IO.File]::WriteAllText("$EV\cfg\deny\config.toml",  "[security]`nenabled = true`negress_allow = []`n",                    $utf8)

$bytes = New-Object byte[] 16
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
$NONCE = ($bytes | ForEach-Object { $_.ToString("x2") }) -join ""

# NO provider API key is set anywhere in this run - that is now part of what is
# being proven. The only env value supplied is a deliberately-INVALID cloud token,
# which is required by design: CloudCredential::from_env rejects only the EMPTY
# string, and without a present value the product short-circuits on
# CredentialAbsent before any socket opens, which is what made the previous
# Windows evidence vacuous.
Remove-Item Env:\ANTHROPIC_API_KEY -EA SilentlyContinue
Remove-Item Env:\OPENAI_API_KEY    -EA SilentlyContinue
Remove-Item Env:\API_KEY           -EA SilentlyContinue
$env:WAYLAND_F25_CLOUD_TOKEN = "INVALID-PLACEHOLDER-NOT-A-CREDENTIAL-25c4-windows"
$env:WAYLAND_F25_CLOUD_ORG   = "wayland-f25-proof"

$meta = @()
$meta += "HOST=$(hostname)"
$meta += "UTC=$([DateTime]::UtcNow.ToString('o'))"
$meta += "FIXED_BINARY=$FIXED"
$meta += "FIXED_SHA256=$((Get-FileHash $FIXED -Algorithm SHA256).Hash)"
$meta += "FIXED_COMMIT=$(git -C D:\lane-25c4-win rev-parse HEAD)"
$meta += "BASE_BINARY=$BASE"
$meta += "BASE_VERSION=$(& $BASE --version 2>&1)"
$meta += "BASE_SHA256=$((Get-FileHash $BASE -Algorithm SHA256).Hash)"
$meta += "NONCE_LEN=$($NONCE.Length)"
$meta += "PROVIDER_KEY_SET=$([bool]$env:ANTHROPIC_API_KEY)"
$meta += "URL_SHAPE=/apps/$($env:WAYLAND_F25_CLOUD_ORG)/machines?metadata.wayland_task_nonce=<32-hex>"

$legs = @(
    @{ name = "FIXED-allow";     exe = $FIXED; arm = "allow"; args = @("backend","orphans","--nonce",$NONCE) },
    @{ name = "FIXED-deny";      exe = $FIXED; arm = "deny";  args = @("backend","orphans","--nonce",$NONCE) },
    @{ name = "BASE-deny";       exe = $BASE;  arm = "deny";  args = @("backend","orphans","--nonce",$NONCE) },
    @{ name = "FIXED-list-nokey";exe = $FIXED; arm = "allow"; args = @("backend","list") },
    @{ name = "BASE-list-nokey"; exe = $BASE;  arm = "allow"; args = @("backend","list") }
)
$codes = @{}
foreach ($leg in $legs) {
    $env:WAYLAND_HOME = "$EV\cfg\$($leg.arm)"
    $out = & $leg.exe $leg.args 2>&1 | Out-String
    $codes[$leg.name] = $LASTEXITCODE
    [IO.File]::WriteAllText("$EV\leg-$($leg.name).txt", $out, $utf8)
}

try {
    $r = Invoke-WebRequest -Uri "https://api.machines.dev/v1/apps/wayland-f25-proof/machines" -Headers @{ Authorization = "Bearer INVALID-PLACEHOLDER-NOT-A-CREDENTIAL-25c4-windows" } -UseBasicParsing -TimeoutSec 30
    $netcode = $r.StatusCode
} catch { $netcode = $_.Exception.Response.StatusCode.value__ }
$meta += "NET_CONTROL_HTTP=$netcode"
[IO.File]::WriteAllLines("$EV\proof-meta.txt", $meta, $utf8)

$lines = @()
foreach ($leg in $legs) { $lines += "WLRC_$($leg.name)=$($codes[$leg.name])" }
$lines += "WLDONE"
[IO.File]::WriteAllLines($status, $lines, $utf8)
