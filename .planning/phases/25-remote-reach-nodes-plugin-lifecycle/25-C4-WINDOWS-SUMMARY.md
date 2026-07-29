# Phase 25 Criterion 4 — lane `25-c4-windows` SUMMARY

> *"Compromised keys/plugins/backends and denied secret/**egress** paths fail closed with
> no orphaned execution."* — the Windows half of Half A.

Graded **MET** for the egress-denial clause on Windows, 2026-07-29. Lane branch
`lane/25-c4-windows`, based on integration `gh/plan/f20-unified-audit-repair` @ `632ad619`
(merged forward at lane start; fast-forward, no conflicts).

**Verdict: the gap `25-c4-egress` left open — "no egress-policy denial on Windows" — is
closed, with a positive control that provably reached Fly's servers and an anti-vacuity
control showing the pre-fix binary fails open on the identical config.**

---

## 1. Both arms, verbatim

Everything ran through the shipped CLI on `SeanDesktop` at commit `fa16cb53`, binary
`D:\lane-25c4-win\target\debug\wayland-core.exe`
(SHA256 `B05FD40B…5264F8`). One command; one variable between the arms:
`[security] egress_allow`. Full capture: `evidence/25-c4-windows/25-c4-windows-5leg.txt`.

### DENIAL ARM — `egress_allow = []`

```
INFO egress security ENFORCING — exfil-shaped traffic to non-allowlisted external hosts is blocked allowlisted=36
cloud      enumerated=false found=0 via vendor machine listing failed: egress denied: GET with a
long or high-entropy path/query to a non-allowlisted host. Egress to `api.machines.dev` is blocked
by the security policy. Add it under `[security] egress_allow = [..]` in your config, or disable
the policy entirely with `[security] enabled = false` if you accept the exfiltration risk.
WLRC_FIXED-deny=0
```

### POSITIVE CONTROL — `egress_allow = ["api.machines.dev"]`

```
INFO egress security ENFORCING — exfil-shaped traffic to non-allowlisted external hosts is blocked allowlisted=38
cloud      enumerated=false found=0 via vendor machine listing failed: machine listing returned
HTTP 401: {"error":"Authenticate: token validation error"}
WLRC_FIXED-allow=0
```

**The traffic genuinely left the machine and a remote server answered.** That JSON is Fly's
own body, and in `wcore-exec-backend/src/backends/cloud.rs::api_get` the string
`machine listing returned HTTP {status}` is reachable only after
`client.get(..).send().await` returned and `response.status()` was read. `allowlisted=38`
vs `36` is the identical delta hetzner recorded from the identical pair of configs.

### ANTI-VACUITY CONTROL — pre-fix binary, SAME deny config

```
(no "egress security ENFORCING" line at all)
cloud      enumerated=false found=0 via vendor machine listing failed: machine listing returned
HTTP 401: {"error":"Authenticate: token validation error"}
WLRC_BASE-deny=0
```

`wayland-core 0.12.25` (SHA256 `96E29E52…C25751`), handed `egress_allow = []`, sent the
request anyway. **This is the fail-open the fix closes, demonstrated on Windows**, and it is
what makes the denial arm mean something: the gate can fail, and here is it failing.

### The two controls behind the controls

- **Independent network control:** `Invoke-WebRequest` to the same URL from the same script,
  outside the product — `NET_CONTROL_HTTP=401`. The denial cannot be blamed on a dead network.
- **TLS peer identity** (`evidence/25-c4-windows/tls-peer.txt`), which neither this phase's
  Linux proof nor its Windows one had ever measured:
  `REMOTE_ENDPOINT=[2a09:8280:1::8969]:443` — a **public** IPv6 in Fly's `2a09:8280::/29`,
  reached from the host's own global address; `CERT_SUBJECT=CN=api.machines.dev` issued by
  Let's Encrypt with `SSL_POLICY_ERRORS=None`; no proxy at process, user, machine or WinINET
  scope. This excludes the localhost-proxy / TLS-interceptor / hosts-redirect story.

---

## 2. Where Windows diverges from Linux, and why

| # | divergence | why | how handled |
|---|---|---|---|
| 1 | **Config isolation.** Linux keyed the arms off `XDG_CONFIG_HOME`. | That works on Linux only because `dirs::config_dir()` honours XDG there; on Windows it is `%APPDATA%`. | Used `WAYLAND_HOME`, whose branch is **first** in the existing `wcore_config::config::wayland_config_dir()` on every platform. No new platform branch — see §3. |
| 2 | **The credential.** Linux held a valid Fly token; Windows holds none and none was moved there. | Two hosts, one credential, and they cannot reach each other. | Present-but-invalid placeholder. `CloudCredential::from_env` rejects only the EMPTY string, so the socket still opens; Fly answers **401** where hetzner got **404**. Cross-audited — §5. |
| 3 | **Docker surface unavailable.** Linux enumerated containers; Windows reports `open //./pipe/dockerDesktopLinuxEngine: The system cannot find the file specified`. | Docker Desktop is not running on `seandesktop`. | Left as-is. The product refuses to launder it: `enumerated=false`, and the total line says *"3 surface(s) could NOT be enumerated … must never be read as zero orphans."* Correct behaviour, so nothing to fix — but it means the Windows orphan sweep covers 1 surface where Linux covered 2. |
| 4 | **A pre-existing `wcore-egress` test fails on Windows.** | Not the product — the platform. §4. | Reported RED, deliberately not "fixed". |
| 5 | **Exit status collapses over ssh+PowerShell.** | Known transport property. | Every code in this lane came from a status file (`WLRC_*` first, `WLDONE` last) read back by a separate ssh call. |
| 6 | **rustc `STATUS_ACCESS_VIOLATION` (0xc0000005)** on the first rebuild. | Compiler crash under incremental compilation; not a source error. | Re-ran with `CARGO_INCREMENTAL=0`; clean. Worth knowing for anyone building this workspace on Windows. |

---

## 3. Platform centralization — the fix is to add NOTHING

AGENTS.md requires platform differences to live in one function. An absence claim, so with a
liveness control in the same tree:

```
$ /usr/bin/grep -rc "fn " --include="*.rs" crates/wcore-egress/    # KNOWN-POSITIVE
request.rs:14 policy.rs:14 client.rs:32 error.rs:7 lib.rs:22 url_allow.rs:6 observer.rs:23
$ /usr/bin/grep -rn "cfg(windows)"   --include="*.rs" crates/wcore-egress/ ; echo rc=$?   # rc=1, zero matches
$ /usr/bin/grep -rn "cfg(target_os" --include="*.rs" crates/wcore-egress/ ; echo rc=$?   # rc=1, zero matches
```

The egress boundary has **no platform branches at all**, and my change adds none. The one
place the harness needed platform awareness — where the config lives — is already
centralized in `wayland_config_dir()`, so the harness uses that override instead of
branching. Nothing goes through `Command::new("cmd")`; the lane's own PowerShell is harness,
not product.

---

## 4. Gates (`evidence/25-c4-windows/gates.txt`)

Counts read with `/usr/bin/grep` and `Select-String`, never the `rtk`-proxied `cargo`, and
every `N passed / M ignored / K filtered out` field read back.

| gate | result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | **clean**, rc 0 |
| `cargo check --workspace --all-targets` (Windows) | **rc 0**, `Finished dev profile in 1m 43s` |
| `cargo test -p wcore-cli --lib` | **1866 passed; 0 failed; 1 ignored; 0 filtered out** |
| `cargo test -p wcore-exec-backend` | 86 passed; **1 failed**; 0 ignored; 0 filtered out |
| ↳ same tests, `--test-threads=1` | **10 passed; 0 failed; 77 filtered out** |
| `cargo test -p wcore-egress` | 26 passed; **1 failed**; 0 ignored; 0 filtered out |
| ↳ same test, isolated, ×3 | **fails 3/3** — genuine RED, below |
| Fence vs merge-base `632ad619` | **0 lines** in `wcore-cli/src/{lib,main}.rs` |

**`registry::tests::a_recorded_task_is_readable_by_another_caller_and_removable` — harness
mismatch, not a regression.** Those tests set the process-global
`WAYLAND_EXEC_BACKEND_STATE_DIR`, with an in-source comment saying they "are single-threaded
per process **under nextest**, which runs each test in its own process." `cargo test` runs
them on a thread pool, so siblings race. Serialized: 10 passed, 0 failed.

**`wcore-egress::tests::transport_failure_records_one_stable_error_class` — a genuine RED,
reported as one.** It binds a loopback listener, drops it, connects, and asserts the failure
classifies as `Connect`. On Windows it classifies as `Timeout`, 3/3 in isolation. The cause
is the platform, measured with a raw `.NET TcpClient` that contains no wayland-core code:

```
probe 1 port=56492 NO-ANSWER-IN-2000ms elapsed=2042
probe 2 port=56494 NO-ANSWER-IN-2000ms elapsed=2011
probe 3 port=56496 NO-ANSWER-IN-2000ms elapsed=2013
```

Windows sends **no RST** for a just-released loopback ephemeral port; the SYN is dropped, so
the connect times out. The product classified what actually happened, correctly. The test's
premise — *"closed listener must refuse the connection"* — is a Unix assumption. **I did not
touch it:** relaxing the assertion to accept either class would be engineering a green, and
LANE-BRIEF §5 is right that a reported red is worth more. **Pre-existing**: the only source
file this lane changed is `crates/wcore-cli/src/backend.rs`, and `wcore-egress` sits *below*
`wcore-cli` in the crate graph. Filed for BACKLOG as a test-only Unix assumption, MEDIUM.

---

## 5. HIGH finding, found and fixed: the fix had killed the whole `backend` surface

`arm_egress_policy()` — the 25-C4 fix itself — called `Config::resolve()` and propagated its
error. `Config::resolve` fails with **"No API key found"** on any machine with no provider
credential, and `backend` speaks to no provider on any path. Measured, same config, same
command, `backend list`:

| binary | result |
|---|---|
| `0.12.25` (pre-fix) | **exit 0**, full backend table |
| fixed (`feb38088`) | **exit 1**, `resolving config to arm the egress policy for backend: No API key found.` |

So `list`, `probe`, `run`, `cancel`, `orphans` and `scan` were *all* dead on such a host.
This was invisible on the Linux proof host for exactly the reason LANE-BRIEF §3b-ii warns
about: `/root/.wayland/.env` on hetzner injects `ANTHROPIC_API_KEY` into every process.
Evidence: `evidence/25-c4-windows/apikey-coupling.txt`.

**Fixed in two commits, and the first one was wrong** — recorded because the failure is
instructive. `feb38088` collapsed *every* resolve failure to `Config::default()`, whose
`egress_allow` is empty; the allow arm then **DENIED**, because the operator's allowlist had
been silently discarded. Any keyless host would have been unable to allowlist anything.
`fa16cb53` splits the two shapes: `MissingApiKey` — documented in `wcore-config` as a
*recoverable needs-setup* condition, not a config fault — re-resolves with a sentinel
`api_key` so the real `[security]` block still flows through the ordinary merge (`resolve`
does not persist a CLI-supplied key); any **other** error still arms from defaults, i.e.
enforcing with an empty allowlist, which is strictly *stricter* than any resolved config
could be. Fail-closed in both branches, and loud.

Proven live with **no provider key set anywhere** (`PROVIDER_KEY_SET=False`): `backend list`
exits 0, prints the table, and logs `ENFORCING … allowlisted=38` — the operator's
`api.machines.dev` entry in effect.

---

## 6. Second HIGH, unchanged from the Linux lane and worth repeating

The Linux lane reported that the documented two-key interlock for disabling the boundary
(`[security] enabled = false` **plus** `--i-accept-exfil-risk`) has only one key. On this
tree the *message* has been corrected — the Windows binary now says *"disable the policy
entirely with `[security] enabled = false`"*, with the non-existent flag removed, and
`SecurityConfig`'s doc comment now states plainly that the flag does not exist. **The
interlock itself still does not exist.** A config file alone still disables egress
enforcement process-wide. Owner's decision, not a lane's; restated here so it is not lost
now that the misleading text is gone.

---

## 7. Instruments I broke and repaired (LANE-BRIEF §6b-ii)

1. **A build poller that could not fail.** I launched the first Windows build with
   `Start-Process … -WindowStyle Hidden` and graded *"status file absent ⇒ STILL-BUILDING"*.
   It reported `STILL-BUILDING` for **12 polls over ~9 minutes** against a `build.log` frozen
   at exactly 6002 bytes. The build was **dead** — `Start-Process` did not survive the ssh
   session teardown. Confirmed positively, not by absence: zero cargo/rustc processes carried
   this lane's marker (the one live `rustc --crate-name wayland_core` on the box was another
   lane's), and my target tree sat at `files=1474 MB=236` across a 60-second window.
   **Repaired** (`harness/poll.ps1`): never infer running from an absence; require a positive
   liveness signal and grade four states. Three-assertion self-test, run on the box:
   ```
   A1 known-positive : STATE=BUILDING liveness_procs=4
   A2 known-negative : STATE=DEAD  (marker no process can carry)
   A3 old matcher    : OLD=STILL-BUILDING   <- cannot tell A1 from A2
   ```
   And the underlying cause removed, not just detected: builds now run in the foreground of a
   long-lived ssh connection.
2. **A capture that corrupted the product's own text.** PowerShell decodes a native exe's
   stdout with the OEM code page, so the product's em-dashes arrived as `ΓÇö` and a BOM broke
   a `^local` matcher (2 hits across 3 files that all contained the row). The first captures
   were **not verbatim**. Repaired by forcing UTF-8 on the console and writing BOM-less UTF-8;
   re-verified — em-dash counts 4/4/1, `^local` now 1/1/1.

Both were the same family as the thing this criterion is about: an instrument that cannot
distinguish "nothing happened" from "I could not see".

---

## 8. The credential, stated plainly

**No real cloud credential exists on `seandesktop`, none was moved there, and none was
improvised.** Verified by names and lengths only, never values:
`WAYLAND_F25_CLOUD_TOKEN`, `WAYLAND_F25_CLOUD_ORG`, `FLY_API_TOKEN` all
`user_len=0 machine_len=0`; `C:\Users\seand\.wayland-f25-cloud.env exists=False`.

Every leg used the literal string `INVALID-PLACEHOLDER-NOT-A-CREDENTIAL-25c4-windows`, which
is not a credential for anything and is printed here deliberately. It exists solely to escape
`CredentialAbsent`, which fires **before any socket opens** and is what made the previous
Windows evidence vacuous. Nothing secret was read, printed, transmitted, written to disk or
committed.

Cross-audited (`25-c4-windows-PANEL.md`): gemini **EQUIVALENT**, kimi **EQUIVALENT**
(caveated on TLS), codex **WEAKER** (naming local proxy / TLS interception / DNS redirect),
internal adversarial siding with codex that *no* leg in this phase — Linux included — had
ever measured the TLS peer. Majority taken **only after measuring the minority's confounder**:
public Fly IPv6 endpoint, Let's Encrypt cert for `CN=api.machines.dev`, clean chain, no proxy
at any scope. Codex's remaining ask — a packet capture or Fly-side logs — was **not** done,
and is genuinely stronger; what is here excludes on-host fabrication but does not corroborate
from the far side.

---

## 9. What I did NOT do

- **No packet capture and no vendor-side log.** The far-side corroboration codex asked for.
- **Did not repair `transport_failure_records_one_stable_error_class`.** Reported RED with
  the raw-socket evidence. Relaxing it would be engineering a green.
- **Did not implement `--i-accept-exfil-risk`.** Owner's decision, restated not resolved.
- **Did not exercise the denied-POST path.** As on Linux, the denied request is the orphan
  scan's GET, whose high-entropy query trips the product's real `get_carries_data` rule. The
  machine-create POST remains un-denied on both platforms.
- **Did not run the Windows orphan sweep across the container surface** — Docker Desktop is
  not running there. The product refuses to read that as zero, and so do I.
- **Did not run this on hetzner.** No workspace was created there; all compilation was on
  `D:\` on `seandesktop`. `cargo fmt` alone ran on the Mac.
- **Did not** merge, open a PR, tag, release, close an issue, touch `C:\actions-runner-*`,
  or run `wcore-contract generate`.

## 10. Disk

`D:\lane-25c4-win` peaked at **13.99 GB**, of which **13.91 GB was `target/`**;
`D:\lane-25c4-ev` is **0 GB** (27 files). `target/` removed at lane end — see the closing
measurement in the final report. Nothing was created at the root of `C:\`.

## 11. Honest grade

**Criterion 4's egress clause is now MET on Windows as well as Linux**, with a stronger
control set than the Linux leg had: a vendor-answered positive arm, a pre-fix fail-open arm
proving the gate can fail, an out-of-product network control, and a TLS peer identity that
excludes local fabrication. Criterion 4 overall remains **PARTIAL** for reasons this lane did
not own — the un-denied POST path, and the missing `--i-accept-exfil-risk` interlock that is
an owner decision. One HIGH regression introduced by the original fix was found here and
closed here; one genuine platform RED is reported red.
