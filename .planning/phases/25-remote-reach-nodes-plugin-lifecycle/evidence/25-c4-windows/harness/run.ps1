# lane 25-c4-windows — the 2x2 egress proof on Windows.
#
# Reproduces the SHAPE of the Linux proof exactly (lane 25-c4-egress, hetzner):
#   one command, run twice, with ONE variable — the egress allowlist in the config.
#     allow arm: [security] egress_allow = ["api.machines.dev"]   -> vendor must ANSWER
#     deny  arm: [security] egress_allow = []                     -> policy must DENY
#
# Divergence from Linux, deliberate and centralized (see NOTES):
#   Linux keyed the config off XDG_CONFIG_HOME, which works there only because
#   dirs::config_dir() honours XDG on Linux. On Windows dirs::config_dir() is
#   %APPDATA%. The product already centralizes this in
#   wcore_config::config::wayland_config_dir(), whose FIRST branch is $WAYLAND_HOME
#   on every platform. So this harness uses WAYLAND_HOME — no cfg!(windows), no new
#   platform branch anywhere.
#
# Exit status is written to a status file (WLRC first, WLDONE last) and read back by
# a SEPARATE ssh call, because every non-zero collapses to 1 over ssh+PowerShell.
$ErrorActionPreference = "Continue"

$EV     = "D:\lane-25c4-ev"
$B      = "D:\lane-25c4-win\target\debug\wayland-core.exe"
$status = "$EV\status.txt"

Remove-Item $status -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path "$EV\cfg\allow" | Out-Null
New-Item -ItemType Directory -Force -Path "$EV\cfg\deny"  | Out-Null
Set-Location $EV

# --- the two configs: identical but for ONE key -----------------------------
@'
[security]
enabled = true
egress_allow = ["api.machines.dev"]
'@ | Out-File -FilePath "$EV\cfg\allow\config.toml" -Encoding ascii

@'
[security]
enabled = true
egress_allow = []
'@ | Out-File -FilePath "$EV\cfg\deny\config.toml" -Encoding ascii

# --- nonce generated ON the box and never placed on a command line ----------
# (The previous lane filed a FALSE orphan because its nonce was in the outer ssh
#  argv, so the process-table scanner matched the lane's own shell. Generating it
#  here, inside a script file, keeps it out of every argv.)
$bytes = New-Object byte[] 16
[System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
$NONCE = ($bytes | ForEach-Object { $_.ToString("x2") }) -join ""

# --- the token ---------------------------------------------------------------
# NOT a credential. No real credential exists on this host and none was moved here.
# CloudCredential::from_env only rejects EMPTY, so a deliberately-invalid placeholder
# is enough to get past the CredentialAbsent short-circuit and make the product
# actually open a socket. The vendor then answers 401 instead of 404 — the
# evidentiary property ("a remote server answered") is identical.
$env:WAYLAND_F25_CLOUD_TOKEN = "INVALID-PLACEHOLDER-NOT-A-CREDENTIAL-25c4-windows"
$env:WAYLAND_F25_CLOUD_ORG   = "wayland-f25-proof"

# --- provider key: required by the FIX, not by the command -------------------
# arm_egress_policy() calls Config::resolve(), which hard-fails with "No API key
# found" when no provider credential is configured. `backend orphans` never talks
# to a provider, so this is a coupling the fix introduced. It was invisible on
# Linux only because hetzner's /root/.wayland/.env injects ANTHROPIC_API_KEY into
# every process (LANE-BRIEF §3b-ii). Recorded as a finding; here a clearly-invalid
# placeholder, IDENTICAL in both arms, satisfies config resolution so the arms
# still differ by exactly one variable. No provider request is made on this path.
$env:ANTHROPIC_API_KEY = "INVALID-PLACEHOLDER-NOT-A-CREDENTIAL-25c4-windows"

"HOST=$(hostname)"                              | Out-File "$EV\meta.txt" -Encoding ascii
"UTC=$([DateTime]::UtcNow.ToString('o'))"       | Out-File "$EV\meta.txt" -Encoding ascii -Append
"BINARY=$B"                                     | Out-File "$EV\meta.txt" -Encoding ascii -Append
"BINARY_SHA256=$((Get-FileHash $B -Algorithm SHA256).Hash)" | Out-File "$EV\meta.txt" -Encoding ascii -Append
"COMMIT=$(git -C D:\lane-25c4-win rev-parse HEAD)"          | Out-File "$EV\meta.txt" -Encoding ascii -Append
"NONCE_LEN=$($NONCE.Length)"                    | Out-File "$EV\meta.txt" -Encoding ascii -Append
"URL_SHAPE=/apps/$($env:WAYLAND_F25_CLOUD_ORG)/machines?metadata.wayland_task_nonce=<32-hex>" | Out-File "$EV\meta.txt" -Encoding ascii -Append

$codes = @{}
foreach ($ARM in @("allow","deny")) {
    $env:WAYLAND_HOME = "$EV\cfg\$ARM"
    & $B backend orphans --nonce $NONCE *> "$EV\scan-$ARM.txt"
    $codes[$ARM] = $LASTEXITCODE
}

# --- a network control that does NOT go through the product ------------------
# Proves the box itself can reach api.machines.dev at the moment of the run, so a
# deny result cannot be explained by the network being down.
try {
    $r = Invoke-WebRequest -Uri "https://api.machines.dev/v1/apps/wayland-f25-proof/machines" -Headers @{ Authorization = "Bearer INVALID-PLACEHOLDER-NOT-A-CREDENTIAL-25c4-windows" } -UseBasicParsing -TimeoutSec 30
    $netcode = $r.StatusCode
} catch {
    $netcode = $_.Exception.Response.StatusCode.value__
}
"NET_CONTROL_HTTP=$netcode" | Out-File "$EV\meta.txt" -Encoding ascii -Append

"WLRC_ALLOW=$($codes['allow'])" | Out-File $status -Encoding ascii
"WLRC_DENY=$($codes['deny'])"   | Out-File $status -Encoding ascii -Append
"WLDONE"                        | Out-File $status -Encoding ascii -Append
