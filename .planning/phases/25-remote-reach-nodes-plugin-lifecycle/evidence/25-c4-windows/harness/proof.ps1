# lane 25-c4-windows — consolidated 3-leg egress proof, UTF-8-faithful capture.
#
# CAPTURE DEFECT REPAIRED (2nd instrument repair this lane): PowerShell decodes a
# native exe's stdout using [Console]::OutputEncoding, which defaults to the OEM
# code page (437). The product emits UTF-8, so every non-ASCII byte was mangled on
# the way into the capture file - the product's own em-dashes arrived as "GCo".
# The captures were therefore NOT verbatim. Fixed by forcing UTF-8 on the console
# and writing the files as UTF-8 without a BOM (a BOM also broke a ^local matcher).
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

# Nonce generated ON the box, inside a script file, so it never appears in any argv.
$bytes = New-Object byte[] 16
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
$NONCE = ($bytes | ForEach-Object { $_.ToString("x2") }) -join ""

# Deliberately-INVALID placeholders. Not credentials; no real credential exists on
# this host and none was moved here. Identical in every leg.
$env:WAYLAND_F25_CLOUD_TOKEN = "INVALID-PLACEHOLDER-NOT-A-CREDENTIAL-25c4-windows"
$env:WAYLAND_F25_CLOUD_ORG   = "wayland-f25-proof"
$env:ANTHROPIC_API_KEY       = "INVALID-PLACEHOLDER-NOT-A-CREDENTIAL-25c4-windows"

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
$meta += "URL_SHAPE=/apps/$($env:WAYLAND_F25_CLOUD_ORG)/machines?metadata.wayland_task_nonce=<32-hex>"

$legs = @(
    @{ name = "FIXED-allow";  exe = $FIXED; arm = "allow" },
    @{ name = "FIXED-deny";   exe = $FIXED; arm = "deny"  },
    @{ name = "BASE-deny";    exe = $BASE;  arm = "deny"  }
)
$codes = @{}
foreach ($leg in $legs) {
    $env:WAYLAND_HOME = "$EV\cfg\$($leg.arm)"
    $out = & $leg.exe backend orphans --nonce $NONCE 2>&1 | Out-String
    $codes[$leg.name] = $LASTEXITCODE
    [IO.File]::WriteAllText("$EV\leg-$($leg.name).txt", $out, $utf8)
}

# Network control that does NOT go through the product: proves the box could reach
# api.machines.dev at the time of the run, so a DENY cannot be blamed on the network.
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
