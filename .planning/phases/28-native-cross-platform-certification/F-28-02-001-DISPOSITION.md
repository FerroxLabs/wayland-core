# F-28-02-001 — DISPOSITION

**Finding.** *macOS sandbox activeness is not obtainable through any black-box surface of the
shipped candidate.* 24 `sandbox-probes` cells on `macos` are RED; Criterion 1 is not satisfied
on that family. HIGH by construction under amendment **A2** — the accept and defer paths are
closed, leaving **FIXED** or **DISPROVED** only.

**Disposition: FIXED.**

**Lane:** `lane/macos-activeness`. **Merge-base:** `782ac310`.

---

## 1. Root cause — and what it is NOT

The finding was widely readable as "macOS containment is weak." It is not. The root cause is
an **observability gap, not a containment gap**, and the distinction decides the fix.

macOS containment is real and is applied to every shell command the agent runs:
`wcore-tools/src/bash.rs` selects `default_for_platform()` and executes through
`SandboxBackend::execute`, which on macOS is `SandboxExecBackend` — a `(deny default)` SBPL
profile with an explicit filesystem allowlist and, under `NetworkPolicy::Deny`, no network
rule at all.

What did not exist was any way to **observe** that. Enumerated exhaustively, the surfaces that
run a caller-supplied command were:

| Surface | Runs caller argv? | Applies containment? | Usable for activeness? |
|---|---|---|---|
| `swarm` (delegated) | yes | yes | **no on macOS** — refuses at admission |
| `backend run --backend local` | yes | **no** | no — see below |
| `forge` / `crucible` | yes | yes | no — requires a live LLM provider + credentials |
| agent / `acp` | via the model | yes | no — not deterministic, needs credentials |
| `backend probe` | no | n/a | no — reports availability, not activeness |

Two of those deserve their reason stated rather than asserted:

**`swarm` refuses on macOS, and that refusal is correct.**
`wcore-swarm/src/dispatch.rs::admit_delegated_backend` requires `owns_descendants_hard()`.
`sandbox_exec` does not provide it, so delegated execution is refused with
`sandbox backend sandbox_exec cannot own descendants that escape a process group`. This is a
fail-closed security decision and **it has not been touched.** Admitting `sandbox_exec` to the
delegated path would have turned 24 cells green by weakening a security control — the single
most tempting and most forbidden route available here.

**`backend run --backend local` consults the sandbox but never applies it.** Its own module
doc says so: it "CONSULTS `wcore_sandbox::default_for_platform()` and refuses to run when the
platform has no real containment backend … but it does NOT currently route the child through
`SandboxBackend::execute`." Its receipt therefore reports containment "selected but NOT applied
to this child". This also **explains `F-28-02-005`** (a task run through it created a file
outside its workspace): that path never contained anything, so it evidences nothing about
`sandbox-exec`.

So the product genuinely had no surface through which containment could be observed. That is
the defect, and it is a real product defect independent of certification: this codebase has a
recorded defect class in which the sandbox reports itself available and is silently not applied
(the Windows AppContainer lease wedge, `KR-05` / `F-28-02-002`). **A security boundary an
operator cannot verify is a boundary an operator cannot trust.**

## 2. The fix

`wayland-core sandbox` — a platform-containment operator surface, in
`crates/wcore-cli/src/sandbox_cmd.rs`.

- **`sandbox status [--json]`** — the selected backend and its properties, read straight off
  the live registry.
- **`sandbox exec [--workspace DIR] [--timeout-ms N] "<command>"`** — runs the command through
  the containment path and prints **the child's own output**, so a caller can compare what a
  probe observes inside against what the same probe observes outside and assert activeness from
  the **difference**.

**The property that makes this evidence rather than theatre.** The verb does not re-implement
sandboxed execution. It builds the real `WorkspacePolicy::contained`, selects the backend
through the same fail-closed `SandboxRegistry::required_for_session` an agent session uses, and
dispatches through **`wcore_tools::bash::BashTool::execute_with_ctx` — the agent's own shell
tool, the same function.** A regression that stopped routing the agent's shell through
containment breaks this verb too, so the evidence is **transitive, not parallel.** This was the
unanimous condition attached by all three cross-audit legs (§6) and it is recorded in the
module doc so a later edit cannot quietly dissolve it.

**Not a bypass.** `required_for_session` refuses an explicit `none` selection outright
(`WAYLAND_SANDBOX=none` is an error, not a downgrade) and falls closed to `FailClosedBackend`
where no real containment exists. The child holds strictly **less** authority than the caller's
own shell.

**Harness.** `scripts/f28-native-matrix.mjs::captureActiveness` tries the delegated path
**first, unchanged**, and falls back to `sandbox exec` only when no worker can be spawned at
all. Linux and Windows readings are therefore untouched. The activeness rule is not relaxed:
the differential and its signals are unchanged, a run showing no difference is still
`observed:false`, and a family with no obtainable difference is still RED. The surface that
produced the inside reading is recorded in the detail string.

An `/etc` read signal was added because the three existing signals are all namespace-derived
and **macOS has no PID or mount namespace** — `NSpid` and the root listing are identical inside
and out there, which would have left DNS as the sole differential. It is inert on the other two
families (Linux bwrap read-binds `/etc`; Windows has no `/etc`), so it can only ever *add* a
difference.

## 3. Evidence

### 3.1 Red before / green after

| | Before | After |
|---|---|---|
| macOS inside reading | **unobtainable** — `sandbox_exec cannot own descendants…; docker backend disabled (feature \`live-docker\` off)` | obtainable via `sandbox exec` (§3.3) |
| Surface exercised | none | `BashTool::execute_with_ctx`, the agent's own shell path |

### 3.2 Linux (`hetzner-dsm`, bubblewrap) — the new surface reproduces the delegated path's observation

Run at `8a09297b`, release binary built on the host.

```
OUTSIDE  F28_NSPID=NSpid:3785445  ROOTLS=40 entries  F28_DNS=RESOLVES   host escape marker: PRESENT
INSIDE   F28_NSPID=NSpid:5        ROOTLS=10 entries  F28_DNS=NO_DNS     host escape marker: ABSENT
```

Four independent differences. The first three are **exactly** the three 28-02 recorded for the
Linux family through the delegated path, so the new surface is equivalent evidence rather than
weaker evidence. `sandbox status --json` reported
`{"backend":"bubblewrap","available":true,"bypasses_containment":false,"enforces_read_deny":true,"owns_descendants_hard":true,…}`.

The fourth signal carries its own lesson: the child **reported** `F28_ESCAPE=WROTE_OUTSIDE`,
and only the host-side check showed the file was absent. The child's self-report is not
evidence of anything; the host observation is.

### 3.3 macOS — see §7

### 3.4 Gates, with the reds they are able to produce

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` (Mac) | clean |
| `cargo clippy -p wcore-cli --all-targets -- -D warnings` (hetzner) | **0 warnings, exit 0** |
| `cargo nextest run -p wcore-cli --lib -E 'test(sandbox_cmd)'` | **6 run, 6 passed** |
| `cargo nextest run -p wcore-cli --test sandbox_activeness` | **2 run, 2 passed** |
| `cargo nextest run -p wcore-eval-scenarios` | **326 run, 326 passed, 5 skipped** |
| `node scripts/f28-native-matrix.mjs --self-test` | **25 assertions, 0 failed** |

**Every gate above was made to go red.** Not asserted — executed:

1. **The e2e containment gate went red on its first real run** — and the red was correct
   behaviour from a wrong test. It sited its escape target in a `tempfile::tempdir()`, and
   `WorkspacePolicy::contained` deliberately grants the whole of `std::env::temp_dir()` as a
   scratch root, so the child's write there was authorised. **The test was fixed, not the
   assertion weakened**, and the episode is recorded in a comment in the test so a future
   reader does not "fix" it the other way.
2. **Mutation: the invocation was rewired to run the probe through a raw shell instead of
   `sandbox exec`.** Result: `1 passed, 1 failed` — the differential assertion fired. Source
   restored, `git diff` empty. This proves the gate is sensitive to *whether execution went
   through the sandbox*, which is the only thing it is there to detect.
3. The unit gate went red on a real compile error (`SandboxRegistry` is not `Debug`), fixed at
   `951c9f7c`.

**Two vacuous-pass traps the e2e gate is explicitly built around**, both of which would have
made it self-passing:
- *Absence of the escape file is not evidence of containment.* A sandbox that failed to launch
  produces the same absence. The child must print `F28_CHILD_RAN` before the absence counts.
- *A host with no backend must not pass silently.* There is no skip branch: such a host takes
  the other assert — `sandbox exec` must refuse.

## 4. Revert-proof

The fix is four commits on `lane/macos-activeness`, and reverting the first restores the
finding exactly:

| Commit | Change |
|---|---|
| `8a09297b` | the surface (`sandbox_cmd.rs` + 21 additive lines across the two fenced files) |
| `951c9f7c` | test-compile fix (inside `#[cfg(test)]`) |
| `2fe25c7e` | e2e gate + escape-target correction |
| `2348ae3d` | harness fallback + `/etc` signal |

Revert `8a09297b` and `wayland-core sandbox` ceases to exist; `captureActiveness` falls back to
a subcommand the binary does not have, `alternate.ran` is false, and the macOS family returns
to `observed: false` with the original reason — 24 RED. Nothing else in the tree changes: the
delegated admission gate, the sandbox backends and the skip taxonomy are untouched.

## 5. What this evidence does NOT cover — stated because it is the strongest objection

`sandbox exec` proves that **the containment path contains**. Black-box, it does not
independently prove that *the agent's autonomous tool calls are routed through that path* —
that rests on the shared call to `BashTool::execute_with_ctx`, which is a source-level fact,
not a black-box one. All three cross-audit legs raised exactly this, and it is the honest limit
of the claim. It is mitigated by construction (one shared function, not two) rather than by
argument, and a stronger black-box proof would require driving a live model, which needs
credentials this lane does not have and must not have.

## 6. Cross-audit

Question: is this a legitimate product FIX or gaming the certification, and does the verb
introduce a security regression? Full prompt: `scratchpad/panel-q.txt`.

| Leg | (1) fix or gaming | (2) security regression |
|---|---|---|
| codex `gpt-5.6-sol` | **legitimate fix**, conditional on using the exact production path | **none**; confused-deputy only if the binary later becomes setuid/privileged |
| gemini `3.1-pro-preview` | **legitimate fix** — "an opaque security boundary is an operational failure" | **none** — "a pure privilege drop" |
| kimi K3 | **legitimate fix**, conditional on sharing the selector/profile code path | **none** |

**Unanimous, 3-0, with one unanimous condition** — share the code path so a shell-tool
regression breaks the probe too. **Condition adopted**: the verb dispatches through
`BashTool::execute_with_ctx` itself.

Internal adversarial pass, arguing against the consensus, produced the one substantive catch:
the condition is necessary but **not sufficient**, because `build_sandbox_pieces(cmd, None)`
yields an *empty* allowlist manifest — more restrictive than production and therefore an
unrepresentative differential. The design was changed in response: the verb builds a real
`WorkspacePolicy::contained` rather than passing `None`. Recorded because it changed the fix.

## 7. macOS live proof

See `F-28-02-001-MACOS-PROOF.md` (written by the same lane against the CI-built
`wayland-core-aarch64-apple-darwin` artifact for this branch).

## 8. Findings raised by this work

| id | Severity | Disposition | Subject |
|---|---|---|---|
| `F-MA-001` | MEDIUM | → BACKLOG | `WorkspacePolicy::contained` grants the **entire host temp directory** as a writable scratch root (`scratch_dirs()`), so a contained shell may write anywhere under `/tmp`. Deliberate and documented in code, contradicts no criterion — recorded, not inflated. Measured, not argued: it is what produced the first red in §3.4. |
| `F-MA-002` | LOW | → BACKLOG | `backend run --backend local` reports a containment backend by name in its effective policy while never applying it. Honest in the receipt text, but it is the surface 28-02 reached for first, and it is what `F-28-02-005` actually measured. |

`F-28-02-006` (bwrap read-binds all of `/etc`, so a sandboxed worker reads `/etc/shadow`) was
**independently reproduced** during §3.2 — `F28_SHADOW=READ` inside the sandbox. Already
recorded by 28-02 at MEDIUM/BACKLOG; not reopened and not re-scored.

## 9. What was NOT done

- **The delegated admission gate was not touched.** `sandbox_exec` still cannot own descendants
  that escape a process group and `swarm` still refuses on macOS.
- **No test was weakened, `#[ignore]`d, `#[allow]`ed, re-gated or deleted; no timeout raised.**
  The one red that appeared was fixed at its cause.
- **No cell was converted to a skip, no skip class invented, and the 651-cell matrix and skip
  taxonomy were not touched.**
- **The 24 cells were not re-graded.** Re-resolution against the tip belongs to 28-03.
- `crates/wcore-eval-scenarios/src/e5_cases.rs`, `src/lib.rs` and `Cargo.toml` were **not
  touched** — the probe table is unchanged, so the executor/definition mirror test still holds
  (326/326).
- **No `Cargo.toml` or `Cargo.lock` change, no new dependency.**
- **`wcore-contract generate` was not run.** No PR, merge, tag, release or issue closure.
- Neither AppContainer intel file is cited for anything.
