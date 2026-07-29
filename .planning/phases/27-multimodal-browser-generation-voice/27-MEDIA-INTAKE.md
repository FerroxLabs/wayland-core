---
lane: 27-media-intake
criterion: "27-C1 — Standalone and host messages use one bounded, validated attachment/document intake path and degrade explicitly on unsupported providers"
grade-27-C1: PARTIAL (unchanged grade; one of its two clauses materially advanced, the other still RED and reported RED)
vision-reachable: YES — closed and live-proved end to end (was: unreachable by code absence)
new-finding: "HIGH — four user-facing vision remediation strings named only ANTHROPIC/OPENAI/GEMINI; following them verbatim as a FluxRouter user meant setting OPENAI_API_KEY=<flux key>, which the same lane proved misdirects the credential to a third party. Fixed in-lane. Second finding: /root/.wayland/.env injects ANTHROPIC_API_KEY into the process regardless of the shell environment, which silently routes any live vision proof on hetzner-dsm onto arm 1."
credential-disclosure: "A FluxRouter key reached hetzner-dsm. Injected over stdin only (`printf | ssh 'read -r K'`), exported into the child process environment, never in argv, never written to any file, never echoed. Swept with scripts/f24-secret-sweep.sh over all 13 lane artifacts: 0 value hits with the known-positive control alive. Sweep self-test passed 5/5 including the third assertion (the broken invocation misses a planted secret)."
fence-exposure: "ZERO. crates/wcore-cli/src/{lib,main}.rs unchanged vs 861d1b1a (git diff --numstat empty; differ proven alive by 8 other files reporting)."
status: complete
---

# 27-media-intake — vision intake seam closed, "one intake path" still RED

Lane branch `lane/27-media-intake`, base `861d1b1a`. All compilation, tests and
clippy on `hetzner-dsm`; `cargo fmt` on the Mac only.

---

## 1. What I was asked, and what I actually closed

Criterion 1 has two clauses. **They did not both move, and I am not going to
report them as if they did.**

| Clause | Before | After | Evidence |
|---|---|---|---|
| "degrade explicitly on unsupported providers" | image class met (prior lane) | **met, and now reachable at all** | live, below |
| inbound vision reachable | **unreachable by code absence** | **closed, live-proved** | live, below |
| "**one** bounded validated intake path" | RED | **still RED** | measured, §5 |

---

## 2. `BL-F24-C3-H7` — reproduced, then closed

I re-measured rather than inheriting the finding. Unproxied `/usr/bin/grep`,
globs quoted, **with a liveness control in the same invocation**:

```
/usr/bin/grep -rn "build_vision_backend" crates/ --include="*.rs"
  → definition at tool_backends/mod.rs:321, signature takes NO &Config
  → 4 call sites, none able to pass one
/usr/bin/grep -rn "transcription_backend_from_config" crates/ --include="*.rs" | wc -l
  → 7        ← CONTROL. Non-zero, so the instrument discriminates.
```

`OpenAiVisionBackend` posted to a hardcoded `https://api.openai.com/v1/chat/completions`
while accepting a caller-supplied key. **That is why this could not be worked around
with a key substitution:** `OPENAI_API_KEY=<flux key>` would have shipped a FluxRouter
credential to OpenAI — a third party — rather than failing closed. I did not run that
experiment even once.

**The fix** gives `build_vision_backend()` the config seam its sibling
`transcription_backend_from_config` already had, copying that shape rather than
improvising: arms 4 (active OpenAI-wire provider resolved from `Config`) and 5
(`FLUX_API_KEY`), **appended, not prioritised**, so no previously-resolving
configuration changes backend. `OpenAiVisionBackend::new()` still pins `api.openai.com`,
so arms 1-3 are byte-identical.

---

## 3. Live proof — a real image in, a real description out

Real binary, provenance checked against the stale-build class:

```
wayland-core 0.12.25 (source 3159a9c570fedc560a195efd5d320e38162d26d8)
git diff --name-only 3159a9c5 HEAD -- crates/  →  0 files
```

**A trap I hit and had to route around, which is itself a finding.** My first live run
logged `vision: using Anthropic (ANTHROPIC_API_KEY found)` **despite my having unset that
variable in the shell.** `/root/.wayland/.env` on `hetzner-dsm` carries an
`ANTHROPIC_API_KEY`, and the binary loads that file itself at startup, so no shell `unset`
can win. **Had I not read the arm line back, I would have reported a green "live vision
proof" that never touched the code I wrote.** Routed around with an isolated
`WAYLAND_HOME` (I did not edit the shared `.env` — other lanes depend on it).

**POSITIVE** — `evidence/27-media-intake/POSITIVE.txt`, rc=0:

```
INFO vision: using flux-auto at https://api.fluxrouter.ai/v1/chat/completions
     (active OpenAI-wire provider)                      ← ARM 4. The seam I added.

> vision_analyze({"image_url":"/root/27mi-live/vision-fixture.png", ...})
  └> {"analysis":"...**\"VORTHAK\"** ... **\"7492\"** ... A **solid red triangle**
      positioned in the lower-right portion ... pointing upward"}

* Text visible: VORTHAK, 7492
* Shapes present: One red triangle (pointing upward)
```

Ground truth is **unguessable by construction** — `VORTHAK` is not a word, `7492` is not
a default. A model answering blind cannot emit either. Fixture is deterministic and
regenerable (`make-vision-fixture.py`), `sha256=0115a686f37bfcb1eb2d1363562ce53bda4adbc80d8dbd7fae86f983a4ec17a3`,
digest verified identical after transfer to hetzner. **All three ground truths recovered.**

---

## 4. Negative controls — including one that failed and is retained because it failed

**NEGATIVE (credential removed) — I do NOT count this as a vision proof.** Per §3b-i I
asserted the output existed and was non-empty first, then ran the liveness control, and
**the control failed**:

```
STEP 1  non-empty: 581 bytes                     ✓
STEP 2  liveness — "vision-fixture.png" present: 0    ✗ ← FAILED
STEP 3  ground truth "VORTHAK":                  0
```

The zero at step 3 is worthless: the agent aborted at provider construction and **never
reached vision at all**. Removing the credential kills the whole run, so this reddens on
the right variable but measures the wrong thing. **Had I stopped at "0 VORTHAK, reddens
correctly", I would have shipped exactly the self-passing known-negative the brief
warns about.** Retained in evidence as the artifact of the control doing its job.

**NEGATIVE2 — the control that actually isolates vision, on ONE variable**
(`WAYLAND_VISION_MODEL` invalid; credential, provider, base URL, fixture and prompt all
byte-identical to POSITIVE):

```
STEP 1  non-empty: 9057 bytes                          ✓
STEP 2  liveness — "vision-fixture.png" present: 2     ✓  the tool WAS reached
STEP 3  ground truth "VORTHAK":                  0     ✓  and was NOT recovered
        flux-router vision returned HTTP 500

user-visible final output:
  * Error from vision_analyze: Flux Router error: key not allowed to access model.
    This key can only access models=['flux-fast', ... ]
```

**The degradation is observable where it must be** — it lands in the tool result the model
reads *and* in the final user-visible answer, not in a swallowed log line. It is also
labelled **`flux-router`**, not `openai`: the backend previously hardcoded that string, so
before this lane a Flux failure would have reported an untrue provider.

---

## 5. "One bounded validated intake path" — still RED, and I am reporting it RED

The prior verdict graded this RED. I did not take that on trust, and I did not repaint it.
Measured with a control on the same file:

```
/usr/bin/grep -rn "media_intake" crates/ --include="*.rs" | wc -l   → 3   ← control alive
/usr/bin/grep -c "media_intake" crates/wcore-agent/src/channel_media.rs → 0
/usr/bin/grep -c "Attachment"   crates/wcore-agent/src/channel_media.rs → 18  ← control on THAT file
```

`media_intake` lives only in `wcore-tools/src/{lib,pdf_tool}.rs`. The image and channel-media
routes do not go through it. **The mechanism is shared for documents and duplicated for
images and channel media. That clause of C1 remains unmet** and closing it was not
within this lane's cost envelope.

---

## 6. Gates — and proof they can fail

| Gate | Result | Run |
|---|---|---|
| `cargo test -p wcore-agent --lib tool_backends::` | **238 passed; 0 failed; 3 ignored; 1940 filtered out** | isolated |
| 6 new vision tests, `--exact` by name | **6 passed; 0 failed; 0 ignored; 2175 filtered out** | isolated |
| `cargo test -p wcore-agent --lib -- channel_media::` | **14 passed; 0 failed** | isolated |
| `cargo test -p wcore-cli --lib` | **1844 passed; 0 failed; 1 ignored** | isolated |
| `cargo clippy -p wcore-agent -p wcore-cli --lib --all-features` | clean (only pre-existing `imap-proto` future-incompat) | isolated |
| `cargo fmt --all -- --check` | rc=0, 0 lines | Mac |

Counts read back explicitly via `/usr/bin/env cargo` (unproxied) because the proxy strips
`0 ignored` / `0 filtered out` — the exact fields the anti-vacuity rule needs.

**Can the gate fail? Proved by mutation, not asserted.** I reverted
`vision_backend_from_config` to the pre-fix hardcoding (`OPENAI_API_BASE` with the
caller's Flux key — literally the defect) and re-ran:

```
test tool_backends::tests::a_flux_credential_never_resolves_an_openai_host ... FAILED
  a FluxRouter credential resolved endpoint https://api.openai.com/v1/chat/completions
  — that would misdirect the key to a third party (BL-F24-C3-H7)
test result: FAILED. 0 passed; 1 failed; 0 ignored; 2180 filtered out
```

Mutation reverted by file copy, not `git checkout` (other lanes share the object store);
post-restore grep confirms count 1 for the correct form.

### The full-suite cluster is NOT mine — measured at base, not reasoned about

`cargo test -p wcore-agent --lib` (full, parallel, while other lanes build) shows a
failure cluster in `session::` / `session_journal::` / `engine::`. I did not assume it was
environmental:

| Run | Result |
|---|---|
| **base `861d1b1a`**, full parallel | 2156 passed; **16 failed** |
| **mine `3159a9c5`**, full parallel | 2163 passed; **15 failed** |
| **mine**, same commit, re-run | 2163 passed; **17 failed** ← set is not stable |
| the 7 "mine-only" names, run serially at my commit | **7 passed; 0 failed; 0 ignored** |
| `session:: session_journal:: engine::` serially at my commit | **429 passed; 0 failed** |
| vision / `tool_backends` names in either failure set | **0 and 0** |

The set moves between two runs of the *identical* commit, base fails *more* than mine, and
nothing in either set touches vision. This is the contention class LANE-BRIEF §6 documents.
**Reported as pre-existing, with the base number stated so a reader can check me.**

---

## 7. New findings

**HIGH — remediation text that steers the user into the hazard.** Four user-facing strings
named only `ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY`:
`capability_advisory.rs:45` and `:60`, `channel_media.rs:70` (the one that reaches the
**model's prompt** via `Attachment::transcribed` → `build_turn_prompt`, `channel_dispatch.rs:290`),
and the TUI provider list `tui/surfaces/config.rs:1245`. After this change that advice is
not merely incomplete — **a FluxRouter user following it verbatim sets
`OPENAI_API_KEY=<flux key>`, which is precisely the credential misdirection this lane
exists to prevent.** Same defect class as the open browser HIGH (`[browser] allowed_origins`
vs `[browser.policy] allowed_origins`). **All four corrected in the same commit**, per
§6b-ii: repair the instrument in the lane that finds it, do not merely write it up.

**MEDIUM — `/root/.wayland/.env` silently overrides the shell on `hetzner-dsm`.** It carries
`ANTHROPIC_API_KEY`, loaded by the binary itself at startup, so `unset` in the calling shell
does nothing. Any lane doing a live provider-arm proof on that box will exercise arm 1 and
believe it proved its own arm. Route around with an isolated `WAYLAND_HOME`; do not edit
the shared file. → BACKLOG.

---

## 8. What I did NOT do, and refuse to claim

- **I did not make the intake path one path.** §5 measures it as still plural. C1's first
  clause stays RED.
- **I did not live-drive the enricher's `IMAGE_NO_VISION_NOTICE`.** I traced it in source to
  the model's prompt and its unit tests are green (14/14), but **no inbound channel
  attachment was driven live**, so "the degraded notice reaches a real model on a real
  inbound image" is unit-level here, not live. My live degradation proof is the
  `vision_analyze` tool path (§4), which is a different surface.
- **I did not exercise the TUI**, and did not drive the JSON-stream/host surface. C1 names
  "standalone **and host** messages"; I proved standalone only.
- **I did not touch C2, C3, C4 or C5.** They remain NOT MET.
- **The credential-removal negative control does not isolate vision** — stated in §4 rather
  than quietly replaced with the one that worked.
- No `wcore-contract generate`, no PR, no merge, no tag, no issue closed, no workflow edits.

## 9. Seam requests

None. This lane needed no protocol or contract change.
