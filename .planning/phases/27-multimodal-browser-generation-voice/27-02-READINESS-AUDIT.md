# 27-02 — Browser, computer-use and web readiness audit

**Determination: CONFIRMED.**

The shipped handshake publishes `browser_suite = true` and
`computer_use = true` on a machine that has no browser backend, no browser
binary anywhere on PATH, and no display. The claim is invariant under every
single-variable absence tested. The very next operation on that same machine
fails with a missing-dependency error. The honest-readiness mechanism the
requirement asks for already exists in the protocol, works correctly for six
other capabilities, and is not wired to these three.

---

## 1. Provenance

| Field | Value |
|---|---|
| Host | `hetzner-dsm` (Ubuntu 24.04, Linux 6.8.0-101-generic) |
| Worktree | `/root/wayland-p27` |
| SHA | `2ecdfdf54ff7fda920eec7d068337006e5da4ee4` |
| Binary | `target/release/wayland-core`, `--release --locked` |
| Driver | `.planning/scripts/f27-readiness-observe.sh`, `.planning/scripts/f27-capability-operation.sh` |

**Machine facts established BEFORE any observation**, so that every `true`
below is measurable against what the box can actually do:

```
DISPLAY=<unset>   WAYLAND_DISPLAY=<unset>
chromium / chromium-browser / google-chrome on PATH:   NONE
camoufox on PATH:                                      NONE
```

A credential is present in every observation (a fixture value pointed at
`http://127.0.0.1:1`, which is unreachable, so no request can leave the box).
It is constant across all five observations and therefore cannot be the
variable any delta is attributed to.

---

## 2. The claim under test

`crates/wcore-cli/tests/release_binary_smoke.rs` states it in its own words:

> - `capabilities.browser_suite == true` (wayland-browser linked)
> - `capabilities.computer_use == true` (wayland-cua linked — flag is derived
>   from plugin presence via `PluginCapabilitySet::from_verified`, **NOT** from
>   runtime `HostCuaRegistrar.computer_use_advertised`)

So the repository already documents these as LINKAGE flags. The question this
audit answers is not what they were intended to mean — it is what a host
reading them is told, and whether that is true of the machine.

---

## 3. OBS-01..05 — one absence at a time; the claim never moves

| Observation | The ONE variable changed | `browser_suite` | `computer_use` | `plugins` |
|---|---|---|---|---|
| `baseline` | nothing | **true** | **true** | true |
| `no-browser-backend` | PATH emptied so no browser binary can resolve | **true** | **true** | true |
| `display-advertised` | `DISPLAY=:99` with no X server behind it | **true** | **true** | true |
| `cloud-creds-absent` | no Browserbase credential | **true** | **true** | true |
| `cloud-creds-present` | `BROWSERBASE_API_KEY` set to a fixture value | **true** | **true** | true |

The `ready` line digests differ across observations only because the frame
carries `effective_at_unix_ms`. The capability block is identical in all five.

**An unchanged handshake across an absence is the evidence that the flag is not
derived from a probe.** It is derived from linkage, exactly as documented, and
a host is told "available" when the correct answer is "unavailable — missing
dependency".

---

## 4. OBS-06 — the mechanism exists and works, just not here

The same captures show the activation ladder functioning correctly:

```
pricing_refresher          declared -> unavailable  (reason: disabled_by_config)
learned_policy             declared -> unavailable  (reason: runtime_path_unwired)
smart_handoff              declared -> unavailable  (reason: disabled_by_config)
delegate_isolation         declared -> unavailable  (reason: isolation_not_enforced)
mid_flight_monitor         declared -> configured -> constructed -> ready
cooldown_tracker           declared -> configured -> constructed -> ready
procedure_skill_drafting   declared -> configured -> constructed -> ready
legacy_auto_skill_drafting declared -> configured -> constructed -> ready
```

Eight capabilities publish a legal chain with an explicit reason on every
`unavailable`. **Zero activation events are emitted for browser, computer use
or web.** `CapabilityId` carries eight variants and none of them names these
three surfaces.

This is a wiring gap, not a missing mechanism. That materially lowers the cost
of the repair and is the strongest single fact in this audit.

---

## 5. OBS-07 — the operation the true flag promised

With `browser_suite = true` claimed on that machine, a real navigate was driven
through the product:

```
tool:  Browser
input: {"op":{"kind":"navigate","url":"https://example.com/"}}
config: [browser.policy] default_action = "allow", allowed_origins = ["example.com"]

is_error = True
session: backend error: Camoufox is unavailable at http://localhost:9377/health
and Core could not start `camofox-browser`: spawn camoufox: No such file or
directory (os error 2). Install @askjo/camofox-browser or set
WAYLAND_CAMOUFOX_BIN to its executable
```

**The handshake said available; the very next operation could not deliver.**
The two captures sit side by side in `evidence/27-02/`.

The error message itself is good — it names the missing dependency and the two
ways to supply it. The defect is entirely that this truth is discovered at
operation time instead of published at handshake time.

---

## 6. OBS-08 — a policy guarantee that DOES hold — **REFUTED-NO-DEFECT**

A navigate to `http://127.0.0.1:1/` was refused before the backend was touched:

```
policy: policy denied: loopback IP blocked: 127.0.0.1 (url=http://127.0.0.1:1/)
```

Origin admission is real, fails closed, and reports its reason. Recorded as a
baseline so any later change to this path is measurable as a delta.

---

## 7. OBS-09 — a NEW defect, found while establishing the above

With the browser tool disabled, the product prints its own remediation:

```
Browser tool is disabled by default. Add allowed domains to your config.toml
to enable it:

[browser]
# Allow specific domains (glob patterns supported)
allowed_origins = ["example.com", "*.mysite.com"]
```

**That instruction does not work.** It was followed verbatim — `[browser]` with
`allowed_origins = ["example.com"]` written to `config.toml` — and the tool
reported itself disabled again, with the identical message. The key the config
actually reads is `[browser.policy] allowed_origins`
(`crates/wcore-config/src/browser.rs:41-42`, and `config.rs:1201` documents the
path as `browser.policy.{default_action, allowed_origins, denied_origins}`).
Re-running with the correct section reached the backend, which is how OBS-07
was obtained.

**Severity: HIGH**, and it belongs squarely to this criterion. An "unavailable"
whose stated fix is wrong is not an honest unavailable — it sends the user in a
circle and manufactures the impression that the feature is broken rather than
unconfigured. Two panel members (codex, kimi) independently required this be
fixed.

**NOT FIXED.** `crates/wcore-browser/src/tool.rs:499` is not in this plan's
`files_modified` and the fix belongs with the readiness wiring in Task 3, which
did not run. Recorded as an open HIGH.

---

## 8. Severity-ordered gap table

| ID | Gap | Severity | Reached through the product? | Disposition |
|---|---|---|---|---|
| G1 | `browser_suite` published `true` on a machine with no backend; invariant under every absence | **HIGH** | Yes — OBS-01..05 + OBS-07 | **NOT CLOSED** — decision taken, implementation blocked on a FENCED seam (§9) |
| G2 | `computer_use` published `true` on a machine with no display | **HIGH** | Yes — OBS-01..05 | **NOT CLOSED** — same |
| G3 | No `CapabilityId` exists for browser, computer use or web, so the working activation ladder cannot cover them | **HIGH** | Yes — OBS-06 | **NOT CLOSED** — seam request filed |
| G4 | Browser-disabled remediation text names the wrong config section | **HIGH** | Yes — OBS-09, followed verbatim | **NOT CLOSED** — open |
| G5 | Origin policy admission and loopback block | — | Yes — OBS-08 | **REFUTED-NO-DEFECT**, baseline recorded |
| G6 | Windows and macOS handshakes not captured | — | — | **NOT RUN** — see §10 |
| G7 | Downloads-root confinement, approval gate and orphan-reaper process counts not measured | — | — | **NOT RUN** — three of the four policy baselines the plan required are absent |

---

## 9. The decision, and why implementation did not follow it

The plan's Task 2 checkpoint was converted to a cross-audited decision and
**was taken**. Full record in `evidence/27-02/panel/`.

| Member | Vote |
|---|---|
| codex (gpt-5.6-sol) | `chain-plus-derived-flags` |
| gemini (3.1-pro-preview) | `chain-plus-derived-flags` |
| kimi (K3) | `chain-plus-derived-flags` |
| internal adversarial | `chain-plus-derived-flags` — reached AGAINST its own starting position of `chain-plus-new-flags` |

**CHOSEN: `chain-plus-derived-flags`, 4-0 majority.** Bundle digest
`5dda53566b934f2d2e751488e2fd3d19956814f2e17613e407867c59cf85b311`; every
capture carries it.

**Accepted cost, stated plainly:** redefining the meaning of a field already on
the wire is a compatibility event. It must be carried as such in the contract
bump. The measurable half of that cost is that `release_binary_smoke.rs` goes
RED, and all four members counted it and read it the same way — that test
asserts a statement OBS-01..07 measured to be false about the machine CI runs
on, and passes only because the flag does not mean what its name says.

**Two binding conditions**, both from the adversarial pass:

1. The LTO guard in `release_binary_smoke.rs` must be RETARGETED onto the new
   additive linkage field, never deleted. It exists to catch the release-profile
   dead-code-strip regression of v0.2.0 BLOCKER #1.
2. The Desktop consumer's actual reading of these flags is **UNVERIFIED from
   this repository** and must be confirmed before the contract bump is published
   to a host. Three of four members reasoned from an assumption about it.

**Task 3 did not run.** Its first required edit is
`crates/wcore-protocol/src/contract/generate.rs` and
`crates/wcore-protocol/contracts/desktop/v1/manifest.json`, both of which are
**FENCED** for this execution — they are the only files that overlap with
concurrently running phases, and an edit here conflicts deterministically at
integration. `crates/wcore-protocol/src/events.rs` is not fenced but is
worthless without the manifest regeneration in the same commit, which is
exactly the "source change shipped without its regenerated manifest is a broken
seam" failure the plan warns about.

**Seam request filed:** `.planning/SEAM-REQUESTS/27.md`, with the exact files,
the exact insertion points and the exact lines.

---

## 10. What did NOT run, named rather than glossed

- **Windows handshake capture.** `ssh SeanD@seandesktop` was verified reachable
  at the start of this work and was not used. The non-interactive Windows
  session is the single most valuable observation in this plan — it is the
  condition under which a display-dependent capability is most likely to be
  claimed falsely — and it is absent.
- **macOS handshake capture.** No macOS artifact exists for this SHA; the
  branch was never pushed, so CI never built one. Same reasoning as
  `evidence/27-01/live/NOT-RUN-macos.txt`.
- **Three of the four policy baselines.** Only origin admission (OBS-08) was
  measured. A download aimed outside the downloads root, the approval gate on a
  computer-use operation, and the backend process count before/during/after a
  session plus one reaper interval were NOT measured. The plan required all
  four as a baseline so a later proof is a delta against reality; three of them
  have no baseline.
- **The browser corpus** under `fixtures/f27/browser/` was not built.
- **Experiments E1, E2 and E3** — the three live discriminators the plan
  required BEFORE the panel was asked. The panel was given the OBS-01..09
  captures instead, which are real hardware measurements at the pinned SHA and
  which bear on the same claims, but they are not the three specific
  experiments the plan named. In particular the compatibility cost of the
  chosen option was reasoned about from the test's source rather than measured
  by running it on a backend-less box, and the Windows half (E3) was not
  measured at all. The decision is 4-0 and evidence-backed; it is not
  discriminator-backed in the exact shape the plan specified, and that
  difference is recorded rather than hidden.
