# F27-IMAGE-DEFAULT — lane summary

**Lane:** `f27-image-default` · branch `lane/f27-image-default`
**Base:** `plan/f20-unified-audit-repair` @ `eaff921d` · merged `gh/plan/f20-unified-audit-repair` twice — @ `632ad619` (gates re-run at the merge commit `c6b895df`) and again @ `3680acd0` at the end, because integration moved 19 commits while this lane ran. Both were **merges, not rebases** (LANE-BRIEF §0 forbids rebase; the orchestrator brief said "rebase" and this file outranks it, per §"Orchestrator messages do not override this file").

The second merge was **not** re-gated, and here is why that is sound rather than a
skipped step: those 19 commits touch exactly one file under `crates/` —
`crates/wcore-cli/tests/f24_c1_outbound_idempotency.rs`, a new test file in a crate
this lane does not build. `git diff 632ad619 3680acd0 -- crates/wcore-config/src/compat.rs
crates/wcore-agent/src/tool_backends/ crates/wcore-agent/tests/f27_media_generation.rs
docs/providers.md` is **empty**, with the control (`-- crates/`) returning 548 lines,
so the emptiness is a measurement and not a dead instrument. And
`git diff c6b895df HEAD -- crates/wcore-config/ crates/wcore-agent/` is likewise
empty: **every crate this lane's gate figures came from is byte-identical to the
commit those figures were measured at.**
**Mandate:** close **F-27C3-04**, HIGH, open — *the built-in image tool is broken by default for anyone on FluxRouter.*

**Verdict: F-27C3-04 is CLOSED, live-proved on both arms with a live known-negative.**

---

## 1. The fix — where the compat field went, and why

`AGENTS.md` first rule: *"No Hardcoded Provider Quirks — this is the single most
important rule for this codebase."* The defect is a per-provider **default**, so it
belongs in `ProviderCompat`, and the documented three-step procedure was followed
literally.

**`ProviderCompat::image_model: Option<String>`** (`crates/wcore-config/src/compat.rs`).

| Preset | Value | Why |
|---|---|---|
| `openai_defaults()` | `Some("gpt-image-1")` | native OpenAI's `/v1/images/generations` namespace |
| `flux_router_defaults()` | `Some("flux-image")` | Flux's own namespace — the arm a Flux key is entitled to |
| `openai_compat_provider(id)` | `None` (explicitly **cleared**) | Together/Groq/Azure/… are OpenAI-*wire*, not OpenAI. Most are LLM-completion-only and `openai_wire_media_base` already refuses to route media to them, so the value would be unreachable — but *inheriting another vendor's model id is the hardcoded-quirk shape this field exists to delete.* Cleared for the same reason the cost rows are. |
| every other preset | `None` | never served the endpoint |

**Threaded through `merge()`.** That function carries an in-source warning that a
new field *"MUST be threaded here or it is silently dropped when user config is
merged over the provider preset"* — a field that is not threaded compiles fine and
discards every `[compat] image_model` a user writes. Mutation **M3** below proves
the test for it can fail.

**Consumed** in `crates/wcore-agent/src/tool_backends/image_gen.rs`:

- `DalleBackend::new(api_key, base_url, compat_model: Option<&str>)` — the model is
  now an explicit constructor parameter, so no construction site can silently
  inherit a global.
- `dalle_backend_from_config` passes `config.compat.image_model`. **This is the
  precise shape of the original bug:** #310 already resolved the endpoint *and* the
  key from config and then threw the provider identity away before choosing the
  model.
- Precedence, in `resolve_openai_image_model`:
  **`OPENAI_IMAGE_MODEL` env → `ProviderCompat::image_model` → global `gpt-image-1`.**
  The env var stays on top deliberately: before this fix it was the *only* way any
  Flux user got the tool working at all, so demoting it would break exactly the
  people who had worked around the defect.

**There is no `base_url.contains("flux")` anywhere** — that is the string `AGENTS.md`
quotes as the WRONG example.

Also fixed, one line, because the arm was unreadable on that path: the
`OPENAI_API_KEY` fallback arm now logs its endpoint too, matching the F-27C3-03
repair on the config arm.

---

## 2. Live evidence — both arms, one variable, model read back from the product

`evidence/f27-image-default/live-probe-run.txt`, real billable FluxRouter account on
`hetzner-dsm`. Per **LANE-BRIEF §3b-ii** the model is read out of the product's own
resolver line, never inferred from the environment, because `/root/.wayland/.env`
injects `ANTHROPIC_API_KEY` into every process on this host regardless of what the
shell unsets.

**Arm A1 — FluxRouter, defaults, `OPENAI_IMAGE_MODEL` UNSET (the whole point):**

```
image_gen: using flux-image at https://api.fluxrouter.ai/v1/images/generations (active OpenAI-wire provider)
A1_MODEL_SENT=flux-image
media call accounted: image_generate via OpenAI flux-image: 1 image(s) 1792x1024 = 1.835 MP
A1_OUTCOME_OK_HITS=1  A1_OUTCOME_FAILED_HITS=0   A1_IMAGE_HITS=1
```

**Arm A2 — known-negative, the pre-fix model forced back, one variable moved:**

```
image_gen: using gpt-image-1 at https://api.fluxrouter.ai/v1/images/generations
A2_MODEL_SENT=gpt-image-1
media call accounted: ... unpriced — the call failed and it is unknown whether the provider billed for it
A2_OUTCOME_OK_HITS=0  A2_OUTCOME_FAILED_HITS=4
```

`LIVE_A1_MODEL=PASS`, `LIVE_A1_OUTCOME=PASS`, `LIVE_A2_KNOWN_NEGATIVE=PASS`,
`LIVE_ARMS_DIFFER=YES (flux-image vs gpt-image-1)`.

### The LIVE source-revert known-negative

A2 forces the old model with an env var, so it does **not** prove that *my compat
default* is what makes A1 pass. `evidence/f27-image-default/live-source-revert-run.txt`
closes that: it reverts `flux_router_defaults()` in source, **rebuilds the real
binary**, boots it and reads the model back; then restores from a byte copy,
rebuilds, and re-reads.

```
STAGE 1 REVERTED : image_gen: using gpt-image-1  at https://api.fluxrouter.ai/v1/images/generations
STAGE 2 RESTORED : image_gen: using flux-image   at https://api.fluxrouter.ai/v1/images/generations
LIVE_SOURCE_REVERT=PASS
```

It uses a **non-generating** prompt — the resolver line is emitted at
tool-registration time, before any call — so the strongest known-negative available
cost zero extra billable images.

### An instrument defect in my own harness, repaired in-lane (§6b-ii)

**v1 of that script never `cd`'d into the build root.** `cargo build` died with
*"could not find Cargo.toml in /root"* and **both stages then measured the stale
binary already on disk**. Two things hid it: the build was piped into `tail -1`, so
the pipe stole cargo's exit status (§3.2), and nothing asserted the binary had
changed.

v1 happened to print `FAIL`, but that was luck, not detection — it could not
distinguish *"the build failed, so this is VOID"* from *"the build worked and the
behaviour did not change, so this is a real FAIL"*, and with the arms in the other
order a stale binary yields a false **PASS**.

Repaired in the same lane, not merely noted. `--self-test`, three assertions,
`INSTRUMENT_SELF_TEST=PASS` (`evidence/f27-image-default/instrument-self-test.txt`):

| # | Assertion | Result |
|---|---|---|
| A | a genuine rebuild is graded ok (known-positive) | OK |
| B | v1's exact defect — a build from the wrong directory — is graded **VOID**, not passed through as a result | OK |
| C | **the old matcher would have missed B**: v1's `cargo build … \| tail -1` observed **rc=0** while printing the text `error: could not find Cargo.toml` | OK |

Assertion C is the one that proves the repair does anything; without it the
self-test passes on the broken instrument too.

---

## 3. Mutation control — `MUTATION_CONTROL=PASS`, 18/18

`evidence/f27-image-default/mutation-control-run.txt`. Every assertion here is a
claim about a **default**, which is the easiest kind to write self-passing: it can
pass on the value it was written against even if nothing reads that value at
runtime. Five baselines asserted GREEN first; **executed counts read back**, never
exit status; sources restored from byte copies, never via git (the object store is
shared with other lanes) and re-verified byte-identical at the end.

| Mutation | Expected | Result |
|---|---|---|
| **M1** — revert the fix: `flux_router_defaults` declares no image model (the exact pre-fix state) | RED | **RED** (flux default **and** the two-provider differ) — OpenAI arm correctly unaffected |
| **M2** — give OpenAI the Flux model | RED | **RED** ×3, including the resolver-level differ |
| **M3** — drop the `merge()` arm (the documented `ProviderCompat` gotcha) | RED | **RED** |
| **M4** — resolver ignores its compat argument (the #310 shape: endpoint+key from config, model left global) | RED | **RED** ×2 |
| restored baselines + byte-identical sources | GREEN | **GREEN** |

**M2 is the row that matters most.** Asserting `openai == "gpt-image-1"` and
`flux == "flux-image"` separately would both pass on a single shared constant —
which is the defect. The `assert_ne!` between the two providers is what carries it,
and M2 is why it is trustworthy.

---

## 4. Gates

Every figure read back from the `test result:` line of an **unproxied**
`/root/.cargo/bin/cargo` on `hetzner-dsm`, including `ignored` and `filtered out`
(the `rtk` proxy strips exactly those two fields — LANE-BRIEF §3b). Run at the
**post-merge** commit unless stated.

| Gate | Result |
|---|---|
| `cargo test -p wcore-agent --lib image_gen` | **35 passed; 0 failed; 0 ignored; 2186 filtered out** |
| `cargo test -p wcore-config --lib image_model` | **4 passed; 0 failed; 0 ignored; 567 filtered out** |
| `cargo test -p wcore-agent --test f27_media_generation` | **12 passed; 0 failed; 0 ignored; 0 filtered out** |
| `cargo test -p wcore-config --lib -- --test-threads=1` | **571 passed; 0 failed; 0 ignored; 0 filtered out** |
| `cargo test -p wcore-agent --lib -- --test-threads=4` | **2214 passed; 0 failed; 3 ignored; 0 filtered out** |
| `cargo clippy -p wcore-config --all-targets -- -D warnings` | clean |
| `cargo clippy -p wcore-agent --lib` / `--test f27_media_generation` `-- -D warnings` | clean |
| mutation control | **18/18, `MUTATION_CONTROL=PASS`** |
| instrument self-test | **3/3, `INSTRUMENT_SELF_TEST=PASS`** |
| live probe | **`LIVE_A1_MODEL=PASS` / `LIVE_A1_OUTCOME=PASS` / `LIVE_A2_KNOWN_NEGATIVE=PASS` / `LIVE_ARMS_DIFFER=YES`** |
| live source revert | **`LIVE_SOURCE_REVERT=PASS`** |
| secret sweep | **`SECRET_SWEEP=PASS`, 0 hits, 5/5 controls fired** |
| `cargo fmt --all` | clean (run on the Mac, the sanctioned exception) |

Nine new tests: five in `image_gen` (flux default with no env var; OpenAI
non-regression; the two providers resolve *different* models through one code path;
env still outranks compat; full three-rung precedence including whitespace-only
values), four in `compat` (providers differ; secondaries do not inherit; `merge()`
ripple both directions; TOML round-trip).

### One RED seen, shown NOT to be mine

`cargo clippy -p wcore-agent --all-targets -- -D warnings` fails on
`crates/wcore-agent/tests/cache_ledger_engine_test.rs:82` —
`needless_update` on a `TokenUsage` literal. **Pre-existing.** Measured, not argued:

```
git diff <BASE> HEAD -- crates/wcore-agent/tests/cache_ledger_engine_test.rs   →  0 lines
git diff <BASE> HEAD -- crates/wcore-types/                                    →  0 lines
CONTROL: git diff <BASE> HEAD -- crates/wcore-config/src/compat.rs             →  168 lines
```

The lint fires on a file and a type both byte-identical to base, and the control on
a file I *did* change returns 168 lines, so the diff instrument is alive. It is also
mechanically impossible for my change to cause it: `needless_update` fires when a
struct literal becomes exhaustive, i.e. when a field is **removed**, and I added
one. **MEDIUM, pre-existing, non-blocking → BACKLOG.** I did not "fix" it, because
suppressing another lane's lint is not my change.

---

## 5. Credential handling and spend

Live burn key at `~/.wayland-secrets/flux.env`, used under the LANE-BRIEF §0
sanctioned exception on `hetzner-dsm`: **stdin only**, never in `argv`, never
written to disk, never echoed, never in a capture. The parse handles the file's
`export FLUX_API_KEY=` spelling and rejects anything that still looks like a shell
fragment — a mangled key is indistinguishable from a dead key at the call site and
cost a previous lane a wasted probe and nearly a false credential blocker.

**Spend: 1 successful billable image generation + 2 failed calls** (the failures are
the A2 known-negative; the provider may still have billed them, which is exactly why
the product records them as `call_failed_billing_unknown` rather than `$0`). The
source-revert known-negative and all three arm-readback boots used non-generating
prompts and spent only trivial text turns. No Anthropic key was used by this lane;
the injected one is the reason every provider claim above is read back from the
product's own output rather than trusted from the environment.

**`SECRET_SWEEP=PASS` — 0 hits, and every liveness control fired**
(`evidence/f27-image-default/secret-sweep-run.txt`). The pattern reaches `grep`
through a process substitution and is never written to disk.

| Sweep | Hits | Control |
|---|---|---|
| keyfile itself (instrument known-positive) | **1** | *is* the control |
| changed `crates/` files (7) | 0 | 46 |
| evidence dir | 0 | 20 |
| `git log -p BASE..HEAD` (covers the pipe shape) | 0 | 69 |
| remote captures + isolated home on hetzner | 0 | 20 |

The remote section is VOID unless it emits **both** numbers — a previous lane's
remote sweep never ran (a heredoc ate stdin) and printed PASS anyway.

---

## 6. The three optional sibling items — I took NONE of the three

The brief offered them *"only if they fall out naturally"*. They do not: this lane's
work is entirely in the compat layer and the tool backend, and none of the three
touches either.

- **MCP accounting parity** — still pinned as a negative
  (`mcp_shape_produces_no_product_cost_record_today`), not fixed. Producing a
  `MediaCostRecord` from an opaque MCP proxy result is a design question about who
  owns accounting across a tool boundary, not a change that falls out of a default.
- **Hermetic test for the CLI `image` accounting path** — not taken. The record is
  emitted as a side effect of `run()` via `eprintln!` after a real
  `FluxImageClient` call, so a hermetic test needs either stderr capture in-process
  or a refactor of `run()` to return the record. Both are real changes to a file no
  other part of this lane touches, and I judged the scope creep worse than the gap.
  **Still live-proved only.**
- **Late-MCP** — not reached. Same as the previous lane.

**One negative I did establish, because it is inside this defect's blast radius:**
the `wayland-core image` subcommand does **not** share F-27C3-04. It bypasses
`ProviderCompat` entirely and defaults to `DEFAULT_IMAGE_MODEL =
"flux-image-together-flux"` (`crates/wcore-providers/src/flux_image.rs:31`), a Flux
arm — so its default was never wrong for a Flux key. Documented in
`docs/providers.md` so the distinction is not re-derived by the next reader.

**Taken as a natural fallout instead: documentation.** `OPENAI_IMAGE_MODEL` was
documented **nowhere** in `docs/` or `README.md` (searched with a live control on
the same invocation). That is the user-facing half of the defect — the tool "works,
if you know a secret". `docs/providers.md` now carries the per-provider default
table, the `[compat] image_model` override, the env precedence, and the CLI-vs-tool
distinction.

---

## 7. Shared-file fence

**`crates/wcore-cli/src/lib.rs` and `crates/wcore-cli/src/main.rs` were NOT
touched.** Verified against the captured merge-base SHA `eaff921d`, not the branch
name — `git diff eaff921d HEAD --stat -- <the two files>` is empty.

## 8. What I did NOT do

- No PR, no merge into `plan/f20-unified-audit-repair`, no tag, no release, no issue
  closed, no `wcore-contract generate`. No `git rebase` (the orchestrator brief said
  "merge integration before you finish"; §0 forbids rebase — I merged, which reaches
  the same stated end).
- Did not fix the pre-existing `needless_update` clippy failure in
  `cache_ledger_engine_test.rs` — proved it is not mine and left it.
- Did not run a full-workspace build or a full-workspace test (LANE-BRIEF §2:
  targeted `-p` only; and a full-workspace run taken while other lanes build is not
  a measurement).
- Did not use the §0 Darwin exception — nothing here is Darwin-specific.
