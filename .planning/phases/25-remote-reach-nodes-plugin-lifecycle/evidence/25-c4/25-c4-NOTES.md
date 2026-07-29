# 25-c4-egress — LANE NOTES (append-only, committed continuously)

Lane `25-c4-egress`. Criterion 4: *"Compromised keys/plugins/backends and denied
secret/egress paths fail closed with no orphaned execution."* Graded **PARTIAL** by
`25-PHASE-VERDICT.md` on 2026-07-29.

My assigned gaps from the verdict's costed list: **G4** (real egress policy DENY, both
hosts — the actual missing clause), **G5** (Windows denied-egress re-run as `backend run`
+ correct the false ledger entry), **G6** (compromised-key refusal proven on *identity*
live through the CLI, not only body digest + unit test).

Out of scope by explicit instruction: cancellation and ssh cleanup (lane `25-c1-cleanup`).

---

## M0 — instrument discipline established before any measurement

Per LANE-BRIEF §3b / §3b-i, every number below comes from `/usr/bin/grep` or
`/usr/bin/git`, never the `rtk` proxy. Two proxy artifacts already observed in this lane:

- `find crates/wcore-cli/src -name '*backend*'` returned the literal string
  `1F 1D:` / `./ backend.rs` — `rtk` re-rendered `find` output into a summary form.
  Confirms the brief's warning extends to `find`.
- zsh ate an unquoted `--include=*.rs` (`(eval):1: no matches found`), silently turning
  three greps into no-ops that returned nothing. **Unquoted globs are a false-absence
  generator**, exactly as §3b-i-2 warns.

### M0-a — I nearly filed a false absence in my first 20 minutes

My first search for egress-policy installation in the CLI was:

```
/usr/bin/grep -rn "install_egress\|egress::install\|install(" crates/wcore-cli/src/ | head -20
```

It returned **`anvil.rs:57` only**, and I formed the belief "the CLI installs an egress
policy in exactly one place." That belief was **FALSE**. The `| head -20` truncated the
result set before `main.rs:1859` — the *primary* install site — was reached, because the
over-broad `install(` alternative flooded the first 20 lines with `marketplace.rs`
uninstall/start_install noise.

The corrected concept-level search (§3b-i-3: search the CONCEPT, not one keyword) is:

```
/usr/bin/grep -rn "install_egress_policy\|install_global_policy\|with_default_policy\|policy_from_config" \
  crates/ --include='*.rs' | /usr/bin/grep -v "^crates/wcore-egress/" | /usr/bin/grep -v "/tests/"
```
→ **21 lines, 8 distinct install sites in `wcore-cli` alone.**
Liveness control in the same invocation: `/usr/bin/grep -rn "install_egress_policy" crates/ --include='*.rs' | wc -l` → **14** (non-zero ⇒ instrument alive).

**`head` on a grep whose pattern is over-broad is a false-absence generator.** Recording
it because the verdict I am acting on was itself built on absence claims, and because
§6b-ii says an instrument defect I merely *document* is a defect I have agreed to keep.
Mitigation adopted for the rest of this lane: no `head` on any grep whose result feeds a
claim; narrow the pattern instead, and always print the full match count.

---

## M1 — FINDING (candidate HIGH): `backend` never installs an egress policy

This is, I believe, the *mechanism* behind the verdict's Gap 4a — the verdict recorded
that no egress policy was installed on either proof host and treated that as a missing
*test*. Source reading says it is a missing *wire*, i.e. a product fail-open, not merely
an unexercised proof.

Chain, each link read from source (all `/usr/bin/grep`, unproxied):

1. `crates/wcore-cli/src/main.rs:1430`
   `TopCmd::Backend(args) => match wcore_cli::backend::run(args).await { Ok(())=>Ok(ExitCode::SUCCESS), Err(e)=>{...Ok(ExitCode::FAILURE)} }`
   — the arm **returns an `ExitCode` directly**. Dispatch ends here.

2. `crates/wcore-cli/src/main.rs:1859`
   `wcore_agent::egress::install_egress_policy(&config);`
   — **429 lines later**, and its own comment concedes the shape:
   *"Subcommands that early-return above (acp/swarm/workflow/agent) never reach here"*.
   The comment enumerates four such subcommands. **`backend` is a fifth, and is not
   listed.** `acp`, `workflow`, `crucible` and `anvil` each self-install
   (`acp.rs:373`, `workflow.rs:200`, `crucible.rs:166/363/411`, `anvil.rs:57`).
   `backend.rs` has **no** such call.

3. `crates/wcore-egress/src/policy.rs:133-140` — with nothing installed,
   `GlobalDefaultPolicy::check` matches `GLOBAL_POLICY.get() => None` and returns
   `EgressDecision::Allow`.

4. `crates/wcore-exec-backend/src/backends/cloud.rs:312,346,427` — three
   `wcore_egress::EgressClient::new()` call sites, i.e. the cloud backend's real
   outbound surface, all built with `default_policy()`.

⇒ **Every outbound request the cloud backend makes under `wayland-core backend …` is
evaluated by an allow-all policy.** The egress chokepoint exists, is well-built, and is
simply not armed on this command path.

The code already says this out loud rather than laundering it —
`crates/wcore-exec-backend/src/policy.rs:58-73` `observed_egress_decision()` returns the
literal string `allow-all-default-no-policy-installed` when nothing is installed, with a
comment stating that rendering it as `allow` "would launder an uninstalled boundary into
a deliberate decision." That is the honest disclosure; nobody acted on it.

**Not yet verified (do not treat as established):**
- [ ] that a built binary actually emits `allow-all-default-no-policy-installed` in a
      receipt from `backend run` (read it back from the product's own output — §3b-ii);
- [ ] that arming the policy on this path produces a *policy-level* `Denied`
      distinguishable from the existing `CredentialAbsent` refusal;
- [ ] the positive direction (permitted egress succeeds) — required before any denial
      claim, per my lane instruction that the two runs must differ by exactly ONE variable.

## M2 — why the existing "denied-egress" proof cannot distinguish the two

`cloud.rs:128,135` raise `ExecError::CredentialAbsent` **before** any of the three
`EgressClient::new()` sites is reached. So with no credential the process makes **no
outbound request at all**, and a denial test on that path passes against a build whose
egress boundary is entirely absent. That is the verdict's Gap 4a restated as a
falsifiability property: **the existing denied-egress case would pass on a build with
`wcore-egress` deleted.** It therefore proves nothing about egress.

Corollary for my own work: a credible egress-denial proof MUST reach the network layer,
which means it must get **past** the credential check. Two variables must be separated —
`credential present/absent` and `policy allow/deny`. The proof needs a 2×2, or at minimum
the two runs {credential present + policy allow} and {credential present + policy deny}.

---

## Status ledger (append after every measurement)

| # | class | state | note |
|---|---|---|---|
| 1 | compromised keys | NOT STARTED | G6 — identity vs body digest |
| 2 | compromised plugins | verdict says covered live (25-02) | to re-read, not re-run |
| 3 | compromised backends | verdict says covered live | to re-read |
| 4 | denied secret | covered (CredentialAbsent) | real, but see M2 |
| 5 | denied EGRESS | **candidate product fail-open, M1** | primary target |
| — | no orphaned execution | verdict says MET, best-instrumented in phase | must re-count survivors per class I add |

_Committed at first measurement per LANE-BRIEF §6b-i. Appended continuously._

---

## M3 — M1 CONFIRMED LIVE, and FIXED (commit b64695df)

Read back from the product's own receipt (§3b-ii), same command, only the binary differs:

| binary | `egress_decision` in the receipt |
|---|---|
| base `fd22dbf4` | `allow-all-default-no-policy-installed` |
| fixed `b64695df` | `shared-egress-policy-installed` |

Command both times: `wayland-core backend run --backend local --receipt-out <f>`, exit 0.
Receipt asserted to exist and be non-empty BEFORE reading (2932 / 2925 bytes); liveness
control `grep -c backend_id` → 2 in both. My first read returned empty for both fields —
**not** a product fact, just pretty-print spacing in my pattern. Asserting the file first is
what stopped that becoming a false finding.

Fix: `crates/wcore-cli/src/backend.rs` — `arm_egress_policy()` called at the top of
`backend::run()`, mirroring `acp.rs:373` and `workflow.rs:200`. `acp.rs`'s comment describes
this exact defect class verbatim for `acp serve`; `backend` was simply not on that list.

## M4 — instrument defect FOUND AND REPAIRED (§6b-ii)

My build-completion poll was `pgrep -f "cargo build -p wcore-cli"`. It reported `BUILDING`
for **12 consecutive polls (~5 min) after the build had already finished at 12:51:01** —
because the `bash -c` wrapper carrying my poll command *contains that literal string*, so
`pgrep -f` matched **itself**. A self-matching liveness probe that can never report DONE.

Repaired, not merely noted: the build now appends an explicit completion marker
(`BUILD_MARKER_DONE rc=$?`) and the poll greps for the marker. Three-assertion self-test:
1. known-positive — marker present after a finished build → poll returns DONE. **PASS**
   (`BUILD_MARKER_DONE rc=0`, binary mtime 13:01:17, log shows `Compiling wcore-cli`).
2. known-negative — no marker while building → poll returns BUILDING. **PASS** (marker
   absent until written; `grep ... || echo BUILDING` fallback).
3. **the old matcher would have missed it** — `pgrep -f` returned BUILDING at 12:52–12:57
   on an already-finished build. **PASS, and this one is measured, not argued.**

## M5 — FINDING F-C4-2 (HIGH): the `--i-accept-exfil-risk` interlock does not exist

Three places document a **two-key** interlock for disabling the egress boundary:
- `wcore-config/src/config.rs:223` — "additionally requires the explicit
  `--i-accept-exfil-risk` CLI flag";
- `wcore-agent/src/egress/policy.rs:38` — "Reached only via the config-file
  `[security] enabled = false` **plus** the explicit `--i-accept-exfil-risk` CLI flag";
- `wcore-config/src/config.rs:4292-4296` — "A config `enabled = false` still requires the
  `--i-accept-exfil-risk` CLI flag to be honored (C8), **so the merge can't silently
  disable the boundary**."

Only one key exists. `egress/install.rs:27-41 policy_from_config` branches on
`config.security.enabled` alone and returns `AgentEgressPolicy::disabled()` — no flag is
consulted. **Established from the product, not from an absence grep:**

```
$ ./target/debug/wayland-core --i-accept-exfil-risk --help
error: unexpected argument '--i-accept-exfil-risk' found
```

⇒ A config file alone silently disables the egress boundary process-wide, which is exactly
what the merge comment claims is impossible. **This is not hypothetical on the proof host:**
`/root/.config/wayland-core/config.toml` (hetzner, mtime 2026-07-04) contains
`[security] enabled = false`. That is the real reason the phase recorded "no egress policy
is installed on either proof host" — not an un-run test, a disabled boundary.

Merge semantics are `enabled: global && project` (most-restrictive AND), so a project
`.wayland-core.toml` **cannot** re-enable it. Verified live: with the project config saying
`enabled = true`, the product still logged `egress security DISABLED via [security]
enabled=false`. **Caught only by reading the posture back from the product's own log** —
my two 2×2 arms were silently identical and neither was enforcing. §3b-ii in action.

Isolation adopted instead: `XDG_CONFIG_HOME` per arm. No shared hetzner config was edited
(other lanes use this host).

## M6 — why the cloud 2×2 does not yet yield a DENY, stated honestly

With the policy armed and enforcing, both arms still reach the vendor:

| arm | `egress_allow` | product posture line | outcome |
|---|---|---|---|
| allow | `["api.machines.dev"]` | `ENFORCING … allowlisted=38` | `VendorApiCall` HTTP 404 |
| deny  | `[]`                   | `ENFORCING … allowlisted=36` | `VendorApiCall` HTTP 404 |

The 38-vs-36 delta proves the config difference took effect (exact host + registrable
domain = 2 entries). But the outcome is identical, and the reason is **by design, not a
hole**: `backend run --backend cloud` calls `availability()` first, which is `api_get` — a
plain, short-path GET. `classify` (`egress/classify.rs:208-268`) ranks a data-less GET to a
new destination as `Ask`, and with no consent doorbell wired `resolve_ask` returns
**Allow**. The exfil-class request is `api_post_json` (machine create), and it is never
reached because availability fails first on a vendor 404 (`app not found` — the F25 Fly app
no longer exists).

So the denial I still owe must come from an **exfil-class** request:
`POST/PUT/PATCH`, or a shared-platform host, or a GET carrying a long/high-entropy
path/query (`get_carries_data`). **I will not manufacture one by creating billable Fly
infrastructure**, and I will not claim the probe-GET allow as a denial — that GET being
allowed is correct behaviour.

**NOT YET ESTABLISHED — do not read M3/M5 as closing the egress clause.**

---

## M7 — G4 CLOSED ON LINUX: a real policy-level egress DENY, live

Evidence: `25-c4-egress-2x2.txt`, raw captures `25-c4-scan-{allow,deny}-raw.txt`.
Binary `b64695df` on `hetzner-dsm`. Credential present and identical in both arms.
Command identical in both arms: `wayland-core backend orphans --nonce <32-hex>`.

**The single variable** is one key in an isolated `XDG_CONFIG_HOME` config:

```
allow:  egress_allow = ["api.machines.dev"]     → product logs ENFORCING allowlisted=38
deny:   egress_allow = []                       → product logs ENFORCING allowlisted=36
```

| arm | cloud row, verbatim from the product |
|---|---|
| **allow** | `enumerated=false found=0 via vendor machine listing failed: machine listing returned HTTP 404: {"error":"app not found"}` |
| **deny** | `enumerated=false found=0 via vendor machine listing failed: egress denied: GET with a long or high-entropy path/query to a non-allowlisted host. Egress to \`api.machines.dev\` is blocked by the security policy.` |

**The positive direction is proven first and it is not a stub:** in the allow arm the
request physically left the machine and **Fly's own servers answered HTTP 404**. A vendor
response cannot be produced by a broken build, an absent policy, or a no-op — which is
precisely the property the verdict says the old denied-egress case lacked (it would have
passed against a build with `wcore-egress` deleted, because `CredentialAbsent` fires before
any socket opens). Here the two arms differ by exactly one config key and produce
**vendor-answered** vs **policy-denied**.

Why this request and not the machine-create POST: `backend run --backend cloud` aborts at
`availability()` (a short-path GET, correctly classified `Ask`→Allow) because the F25 Fly
app no longer exists, so the POST is unreachable without creating billable infrastructure —
which I declined to do. The orphan-scan GET carries the nonce in its query
(`/apps/<app>/machines?metadata.wayland_task_nonce=<nonce>`), and a realistic high-entropy
nonce trips `get_carries_data` (`longest_token_run >= HIGH_ENTROPY_TOKEN_LEN = 24`;
nonce length 32). That is the product's real exfil rule firing on a real operator input,
not a contrivance.

### I do NOT claim a fail-open here — and I nearly filed one

My first read of this output was "denied egress yields `found=0` and `EXIT=0` — fail-open."
**That was wrong, and the product is behaving correctly.** Its own summary line refuses the
laundering explicitly:

> `1 orphan(s) found; 2 surface(s) could NOT be enumerated. An un-enumerated surface is not
> a clean surface — a scan that could not run must never be read as zero orphans.`

`enumerated=false` is carried per-row and the summary counts un-enumerated surfaces
separately. The only soft edge is the process exit code being 0, which the verdict already
recorded as a known, accepted limitation. **Filing this as a security escape would have been
a false positive against a correct product**, which is the most expensive error available
here. Recorded as a near-miss, not a finding.

## M8 — MY OWN HARNESS FILED A FALSE ORPHAN, and I caught it

Both 2×2 arms reported `local enumerated=true found=1`. There was **no orphan**.

The local scanner enumerates the host process table for the nonce. My ssh harness set
`NONCE=a7f3…` on its own command line, so **the harness's own shell matched**, and the
scanner counted it. The raw captures show it: the `- process table:` detail line is my own
`bash -c cd /root/wayland-25c4 && … NONCE=a7f3… …`.

Disproof, and note my FIRST control was itself contaminated:

| control | nonce in process argvs beforehand | `local found=` |
|---|---|---|
| 2×2 arms (nonce on my command line) | ≥1 | **1** ← false positive |
| control B (nonce in a file, but literal still in my *outer* ssh command) | **2** (measured) | 1 ← still contaminated |
| control C (nonce generated ON hetzner via `openssl rand -hex 16`, never transmitted) | **0** (measured) | **0** ✅ |

Control C is the clean one: the nonce never crossed the wire and appeared in zero argvs, and
the count went to zero. **The `found=1` was entirely my instrument.**

This is the trap named in my lane instruction — an instrument reporting a product escape
that did not happen. Had I reported "1 orphan survived the egress denial", it would have
been a false security finding on a correct product.

**Harness rule for anyone re-running Phase 25 orphan scans: never put the nonce on a
command line.** Generate it on the target host into a file and read it from there. Any
evidence run that types the nonce into a shell inflates `local found=` by at least one. This
is a caveat on the phase's Half-B positive controls, not a product defect: nonce-based
process-table scanning cannot distinguish a harness from an orphan, and should not try.

## M9 — secret sweep (both credentials), instrument proven in BOTH directions

Required by my lane instruction. My FIRST sweep was vacuous and I caught it: the burn-key
file line is `export FLUX_API_KEY=…`, my regex anchored `^FLUX_API_KEY=`, the needle came
back **empty**, and `grep -F ""` matched every line of every file — reporting **4 "hits"**
while the liveness control also "passed" trivially, because an empty pattern matches
anything. **An empty needle is a self-passing sweep in both directions at once.**

Repaired: assert the needle is non-empty and ABORT if not; liveness on the real value;
known-negative on the reversed value (proves the sweep is not matching everything).

| needle | len | A) liveness (planted) | B) known-neg (reversed) | C) lane artifacts | D) full tracked diff |
|---|---|---|---|---|---|
| `FLUX_API_KEY` (burn key, Mac) | 51 | 1 ✅ | 0 ✅ | **0** | **0** |
| `WAYLAND_F25_CLOUD_*` (2 values, hetzner) | 641, 12 | 2 ✅ | 0 ✅ | **0** | — |

**Burn-key hit count: 0.** Cloud-credential hit count: 0. Neither value was printed,
echoed, committed or transmitted; the cloud credential was consumed by sourcing the 0600
file on the host that already held it, and never crossed the wire.

---

## M10 — REBASED ONTO INTEGRATION `4a872413` AND RE-PROVEN (orchestrator correction)

I was based on `lane/grade-25`, which predates the 24-branch merge train. All evidence above
was therefore taken on a tree missing 76 changed `crates/` files. **Merged
`gh/plan/f20-unified-audit-repair` @ `4a872413` and re-ran every proof.** Zero merge
conflicts; my change touches exactly one file (`crates/wcore-cli/src/backend.rs`) which is
**unchanged on integration**, so there was nothing to collide with.

**The central defect survives the merge — it is not an artifact of a stale base:**

| | stale base `fd22dbf4` | integration `4a872413` |
|---|---|---|
| `TopCmd::Backend` dispatch | main.rs:1430 | main.rs:**1454** |
| `install_egress_policy` | main.rs:1859 | main.rs:**1896** |
| gap | 429 lines | **442 lines** |

`crates/wcore-egress`, `crates/wcore-agent/src/egress` and `crates/wcore-exec-backend/src`
are **untouched** by the merge train, so every source claim in M1–M9 holds verbatim.

Re-proven on merged commit `7b99b699` (binary rebuilt, `Compiling wcore-cli` = 1):

| proof | merged-tree result |
|---|---|
| receipt `egress_decision` | `shared-egress-policy-installed` (2925 B, liveness `backend_id`→2) |
| egress 2×2 allow | `vendor machine listing failed: … HTTP 404 {"error":"app not found"}` |
| egress 2×2 deny | `egress denied: GET with a long or high-entropy path/query …` |
| identity positive (local vs local) | exit **0** |
| identity negative (container vs local) | exit **1**, `INTEGRITY: OK` + `IDENTITY: REFUSED` |
| identity default (no flag) | exit **0**, integrity-only preserved |

### `gateway support-bundle` — read, NOT duplicated

Checked as instructed. `wcore-gateway/src/support_bundle.rs` implements **Phase 24** SC4 /
T-24-03-01: structural elision first (secret-bearing sources contribute KEY NAMES only),
exact-secret scrubbing as an explicit backstop, `known_secrets` recorded so a reviewer can
spot a bundle whose clean scan is vacuous, and `bundle_files()` exposing the full surface a
canary scan must cover. It is a genuinely good design and its anti-vacuity instinct matches
this programme's.

**My work does not overlap it.** It denies a secret *leaving inside an artifact*; my work
concerns (a) whether the egress boundary is *armed at all* on the `backend` command path and
(b) whether a receipt's *signer identity* is checkable from the CLI. Different mechanisms,
different code, no duplicated verb. I did **not** re-implement any secret scrubbing.

I did not find, in `support_bundle.rs` or `wcore-cli/src/gateway/support.rs`, a hard
post-condition that re-scans the finished bundle and *refuses to hand it over* on a hit —
what I found is the `known_secrets == 0` operator warning and `bundle_files()` as the scan
surface. I may simply have missed it, and I am **not** filing that as an absence: I did not
run the concept-search discipline over it and an unqualified absence claim is the single
easiest thing to get wrong here (§3b-i). Flagged for the owning lane, not graded.

## M11 — regression suite on the merged tree

`cargo test -p wcore-exec-backend` and `-p wcore-cli` at `7b99b699`, counts read back from
the logs with `/usr/bin/grep` (never the `rtk`-proxied `cargo`, which strips `ignored` and
`filtered out`):

```
57 test binaries reporting · 2355 passed · 1 failed · 19 ignored
```

The one failure was **not mine and not a regression**:
`registry::tests::a_recorded_task_is_readable_by_another_caller_and_removable`, in
`wcore-exec-backend` — a crate whose source I did not touch at all (my entire diff is
`crates/wcore-cli/src/backend.rs`). Re-run in isolation at the same commit per §6:

```
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 87 filtered out
```

`1 passed` (not `0 passed`) and `87 filtered out` confirm the test genuinely executed rather
than being filtered away — flavour (c) of the vacuity class. This is the documented shared-
state contention on `hetzner-dsm`; the integration HEAD's own commit message is
*"the anti-vacuity guard was itself vacuous; /tmp is shared on hetzner."*

Also visible in the totals and worth passing on: **one binary reported
`0 passed; 0 failed; 10 ignored`** and another `0 passed; 0 failed; 0 ignored` — suites that
exit 0 having executed nothing. Pre-existing, not mine, but they are live instances of
§3.2's flavour (a) still sitting in the tree.
