#!/usr/bin/env bash
# STEP 1 -- cold install / first run, happy AND unhappy paths.
#
# No credential is used anywhere in this script. It is entirely about what a
# day-one user SEES when they run the binary on a clean machine.
#
# Grading rule for every route: we record rc, stdout, stderr and then judge
# the message on three axes a user actually cares about:
#   (a) does it say what is wrong,
#   (b) does it name a remedy,
#   (c) does the remedy WORK when followed literally.
# (c) is the one this program has been burned by -- a prior lane found the
# headless-keyring error's remedy wrong in three independent ways.
#
# usage: BIN=<path> ./s1-coldstart.sh <outdir>
set -uo pipefail
BIN="${BIN:?set BIN to the wayland-core binary}"
OUT="${1:?usage: $0 <outdir>}"
mkdir -p "$OUT"
RESULTS="$OUT/RESULTS.txt"
: > "$RESULTS"

say() { echo "$*" | tee -a "$RESULTS"; }

# Run one route in a pristine WAYLAND_HOME. Never inherits the caller's env.
# $1 = label, $2 = home dir, rest = argv
route() {
  local label="$1"; shift
  local home="$1"; shift
  local o="$OUT/$label.stdout" e="$OUT/$label.stderr"
  env -i PATH=/usr/bin:/bin HOME="$home/fakehome" WAYLAND_HOME="$home" \
      TERM=dumb NO_COLOR=1 \
      timeout 120 "$BIN" "$@" > "$o" 2> "$e" < /dev/null
  local rc=$?
  say "ROUTE=$label rc=$rc stdout_bytes=$(/usr/bin/wc -c < "$o" | tr -d ' ') stderr_bytes=$(/usr/bin/wc -c < "$e" | tr -d ' ')"
  return $rc
}

say "### environment"
say "keyring present? dbus-launch=$(command -v dbus-launch || echo none) gnome-keyring=$(command -v gnome-keyring-daemon || echo none)"
say "DBUS_SESSION_BUS_ADDRESS=${DBUS_SESSION_BUS_ADDRESS:-<unset>}"
say "binary: $BIN"
say "version: $("$BIN" --version 2>&1 | head -1)"
say ""

# ---------------------------------------------------------------- 1a
# The true day-one cold start: nothing configured at all.
say "### 1a -- bare first run, nothing configured"
H1="$OUT/h1a"; mkdir -p "$H1/fakehome"
route 1a "$H1" "say hi"
say "--- 1a stderr (first 30 lines) ---"
sed 's/^/   | /' "$OUT/1a.stderr" | head -30 | tee -a "$RESULTS" >/dev/null
sed 's/^/   | /' "$OUT/1a.stderr" | head -30
say "--- 1a stdout (first 20 lines) ---"
sed 's/^/   | /' "$OUT/1a.stdout" | head -20
say "artifacts created in WAYLAND_HOME:"
(cd "$H1" && find . -maxdepth 3 | head -40 | sed 's/^/   | /')
say ""

# ---------------------------------------------------------------- 1b-doctor
say "### 1b-doctor -- the 'what is missing' surface on a clean host"
H2="$OUT/h1b"; mkdir -p "$H2/fakehome"
route 1b-doctor "$H2" --doctor
say "--- doctor stdout (first 40 lines) ---"
sed 's/^/   | /' "$OUT/1b-doctor.stdout" | head -40
say ""

# ---------------------------------------------------------------- 1b-init
say "### 1b-init -- scaffolding"
H3="$OUT/h1c"; mkdir -p "$H3/fakehome" "$H3/proj"
( cd "$H3/proj" && env -i PATH=/usr/bin:/bin HOME="$H3/fakehome" WAYLAND_HOME="$H3" TERM=dumb NO_COLOR=1 \
    timeout 60 "$BIN" init > "$OUT/1b-init.stdout" 2> "$OUT/1b-init.stderr" < /dev/null )
say "ROUTE=1b-init rc=$? "
say "files created under proj:"
(cd "$H3/proj" && find . | head -20 | sed 's/^/   | /')
sed 's/^/   | /' "$OUT/1b-init.stdout" | head -15
say ""

# ---------------------------------------------------------------- 1b-badconfig
say "### 1b-badconfig -- malformed config.toml (what does a typo look like?)"
H4="$OUT/h1d"; mkdir -p "$H4/fakehome"
printf '[default\nprovider = "flux-router"\n' > "$H4/config.toml"
route 1b-badconfig "$H4" "say hi"
say "--- stderr ---"
sed 's/^/   | /' "$OUT/1b-badconfig.stderr" | head -15
say ""

# ------------------------------------------------------------- 1b-unknownkey
say "### 1b-unknownkey -- a plausible-but-wrong config key (the silent-ignore class)"
H5="$OUT/h1e"; mkdir -p "$H5/fakehome"
cat > "$H5/config.toml" <<'TOML'
[default]
provider = "flux-router"
model = "flux-fast"

[credentials]
backend = "encrypted-file"

[providrs.flux-router]
base_url = "https://api.fluxrouter.ai/v1"
TOML
route 1b-unknownkey "$H5" "say hi"
say "--- stderr ---"
sed 's/^/   | /' "$OUT/1b-unknownkey.stderr" | head -15
say ""

# ------------------------------------------------------------- 1b-nokey
say "### 1b-nokey -- valid config, provider named, NO credential"
H6="$OUT/h1f"; mkdir -p "$H6/fakehome"
cat > "$H6/config.toml" <<'TOML'
[default]
provider = "flux-router"
model = "flux-fast"

[providers.flux-router]
base_url = "https://api.fluxrouter.ai/v1"
TOML
route 1b-nokey "$H6" "say hi"
say "--- stderr ---"
sed 's/^/   | /' "$OUT/1b-nokey.stderr" | head -20
say ""

# ------------------------------------------------------------ 1b-unwritable
say "### 1b-unwritable -- WAYLAND_HOME the user cannot write"
H7="$OUT/h1g"; mkdir -p "$H7/fakehome"
mkdir -p "$H7/locked"; chmod 500 "$H7/locked"
env -i PATH=/usr/bin:/bin HOME="$H7/fakehome" WAYLAND_HOME="$H7/locked" TERM=dumb NO_COLOR=1 \
    timeout 60 "$BIN" "say hi" > "$OUT/1b-unwritable.stdout" 2> "$OUT/1b-unwritable.stderr" < /dev/null
say "ROUTE=1b-unwritable rc=$?"
say "--- stderr ---"
sed 's/^/   | /' "$OUT/1b-unwritable.stderr" | head -15
chmod 700 "$H7/locked" 2>/dev/null
say ""

say "### step 1 captures written to $OUT"
