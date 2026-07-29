# 27-C2 — readiness A/B on the real shipped binary

**Machine:** `hetzner-dsm` (Ubuntu 24.04 noble, x86-64).
**Tree:** `/root/wayland-27bv`, detached at `861d1b1a716240165209336b1fa38d36f9445716`.
**Binary:** `target/release/wayland-core`, 96,323,064 bytes,
`cargo build --release -p wcore-cli --bin wayland-core`, `Finished in 5m 44s`.
**Both arms are the SAME binary.** Only the probe-input environment differs.
**Runner:** `ab.sh` (committed beside this file).

---

## Why hetzner is a genuine negative control, not a synthetic one

The dispatch requires readiness to be proven *against a box where the capability
genuinely cannot run*. This box is dead in its **natural** state — no env
manipulation is needed to make arm N negative:

```
which camofox-browser camoufox   → (nothing, rc=1)
DISPLAY=[]  WAYLAND_DISPLAY=[]
```

The prior lane's e2e test (`crates/wcore-cli/tests/plugin_discovery_e2e.rs`)
plants a synthetic dead arm. Arm N below plants **nothing**; it is the machine
as it actually is. That is a strictly stronger negative.

---

## Arms

| | Arm N (natural) | Arm R (resolvable) |
|---|---|---|
| `WAYLAND_CAMOUFOX_BIN` | unset | `/bin/true` |
| `DISPLAY` | unset | `:99` |
| `WAYLAND_DISPLAY` | unset | unset |
| `BROWSERBASE_API_KEY` / `_PROJECT_ID` | cleared | cleared |
| `HOME` / `WAYLAND_HOME` / cwd | fresh `mktemp -d` | fresh `mktemp -d` |

Browserbase vars are cleared in **both** arms: a credentialed Browserbase build
probes `Indeterminate` and keeps the flag regardless of any local backend, which
would make arm N silently unfalsifiable.

Invocation (identical in both arms):
```
printf '{"type":"stop"}\n' | timeout 60 wayland-core --json-stream \
    --provider anthropic --api-key test-key-unused
```

Both arms: `rc=0`, 27 stdout lines, 4480 / 4521 bytes.

---

## Result — the `ready` event as a host reads it

`AdvertisedCapabilities` marks these fields `skip_serializing_if = "is_false"`,
so a withdrawn capability is **omitted from the wire entirely**. `<ABSENT>` and
`false` are the same claim: not advertised.

| flag | Arm N | Arm R |
|---|---|---|
| `plugins` | **true** | **true** |
| `browser_suite` | **`<ABSENT>`** | **true** |
| `computer_use` | **`<ABSENT>`** | **true** |

**`plugins: true` in BOTH arms is the anchor.** It is never narrowed, so it
proves the plugin inventory is live in both runs. Without it, arm N's absence
could have been "the plugin was never discovered" — a pass for the wrong
reason. The two arms therefore differ in **backend liveness and nothing else.**

### Arm N narrowed for the right reason, and said so

From `arm-N.stderr` (verbatim, ANSI stripped):

```
WARN not advertising browser_suite: the plugin is loaded but no backend can start
  capability="browser_suite"
  reason=no browser backend can start: `camofox-browser` does not resolve on PATH
         and no sidecar answered http://localhost:9377/health
  remedy=install @askjo/camofox-browser, or set WAYLAND_CAMOUFOX_BIN to the
         executable, or start the Camoufox sidecar before the session

WARN not advertising computer_use: the plugin is loaded but no backend can start
  capability="computer_use"
  reason=neither DISPLAY nor WAYLAND_DISPLAY is set, so no display server is
         reachable and the X11 backend cannot connect
  remedy=run inside a graphical session, or export DISPLAY for an available
         X server (e.g. an Xvfb instance) before starting
```

The narrowing is not silent, and each reason carries an actionable remedy. This
is the recorded panel dissent (a dropped capability becomes an un-debuggable
missing feature) being honoured.

**Verdict on the repair: it is REAL.** The exact state the phase verdict
recorded as broken — `browser_suite`/`computer_use` reading `true` on a box with
no browser binary and no display — is fixed at this SHA, measured on the shipped
binary, against a naturally dead machine.

---

## The residual — Arm R advertises two capabilities that provably cannot work

This is the dispatch's third proof obligation: *a capability that reports `true`
must be shown to actually work, not merely to link.* Arm R shows it does not.

**`browser_suite: true` was granted on `/bin/true`:**
```
/usr/bin/file /bin/true
  → ELF 64-bit LSB pie executable, x86-64 ... stripped
curl -m 3 http://127.0.0.1:9377/health  → http_code=000, rc=7 (connection refused)
```
`/bin/true` exits 0 immediately and serves nothing. `which` resolved it, so the
probe returned `Ready { via: "camoufox-binary" }` and the flag stood.

**`computer_use: true` was granted on a display that does not exist:**
```
ls /tmp/.X11-unix/X99      → No such file or directory
listeners on port 6099     → 0
```
**Instrument liveness control for that zero** (§3b-i — a zero is the success
value here, so the counter must be proven alive):
```
ss -ltn | tail -n +2 | wc -l  → 110
```
`ss` sees 110 listening sockets on this box, so the `0` for `:6099` is a real
measurement and not a dead tool.

### What this means

The repair moved the flag from **linkage** → **resolvability**. It did not move
it to **liveness**. After the repair:

- `browser_suite: true` means *a path resolved on PATH* — satisfied by
  `/bin/true`, by a stale wrapper script, by a partially-installed npm shim, or
  by a binary of the wrong architecture.
- `computer_use: true` on Linux means *a string is set in the environment* —
  satisfied by `DISPLAY=:99` pointing at nothing.

The `DISPLAY` case is **not** a strawman: exporting `DISPLAY=:99` for an Xvfb
instance is the standard CI pattern, and it is exactly what the tool's own
remedy string tells the operator to do. If Xvfb dies, has not started yet, or
never started, `DISPLAY` remains set and the capability is advertised.

**Honest severity assessment.** The dominant real-world case — a headless server
with nothing installed — is now correct, and that was the shipping defect. The
residual needs an operator to have nominated something non-functional. I grade
this **MEDIUM**, not HIGH: it is a genuine gap against C2's honesty bar and it
belongs in BACKLOG, but calling it HIGH would overstate it relative to the
original, where *every* headless box lied with no operator action at all.

---

## Reproduction

```bash
ssh hetzner-dsm
bash /root/wayland-27bv/ab.sh
```
Artifacts land in `/root/wayland-27bv/evidence/arm-{N,R}.{jsonl,stderr}` and are
committed here unmodified.
