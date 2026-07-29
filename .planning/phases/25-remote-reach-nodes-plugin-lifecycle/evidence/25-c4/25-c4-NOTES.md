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
