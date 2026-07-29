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
