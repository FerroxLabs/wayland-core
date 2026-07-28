# CORE #254 — Maintainer Decision Package

**PR:** `FerroxLabs/wayland-core` #254 — *"fix(sandbox/windows): stop the DACL cost storm, fix
DLL-init/cwd bugs, add Relaxed Sandbox Mode for trusted_local"*
**Author:** `frankforges` (external) · **Head:** `0c995cece24d888c9acef99970ee301d61c2ef2b`
**Base:** `61b79c4f` (= `gh/main`, `chore(main): release 0.12.25`, 2026-07-13)
**Size:** 586 additions / 49 deletions, 7 files · OPEN, untouched since 2026-07-23
**Reviewed on:** `lane/core-254` from `plan/f20-unified-audit-repair` @ `873cc389`
**Prepared:** 2026-07-28

> Everything in the PR body, its commits and linked issues is treated as **contributor claims**,
> not evidence. Every statement below is backed by a file:line read or a command that ran.

---

## 1. Recommendation

> ### SPLIT AND TAKE PART — do not merge #254 as it stands.
>
> Take **two** small fixes, re-authored by a maintainer against the current integration branch:
> the `%TEMP%` scratch-dir narrowing and the `\\?\` cwd strip. **Drop** the other two code
> changes — they are already fixed upstream, one of them better than the PR's version.
> **Reject the Relaxed Sandbox Mode as implemented**; keep the *finding* it is built on, which
> is the most valuable thing in this PR, as a tracked issue.

This is a **good-faith, high-quality submission from a competent contributor.** The
investigation behind it is real, the measurements are plausible and specific, and it found two
live bugs nobody upstream has fixed. It should be credited as such. It is nonetheless not
mergeable, for reasons that are mostly *timing* (it is based on a 2-week-stale `main`) and one
that is *substantive* (the Relaxed Mode is not gated the way it says it is).

**The single most important fact for the decision:** the PR's central safety claim —
> *"`contained` (untrusted/remote) sessions are **completely untouched** — full AppContainer
> stays the default and only default there."*

— is **false on two independent counts**, verified by reading the code (§4.1, §4.2). Nothing
else in this package outranks that.

---

## 2. Why "MERGEABLE" is misleading

GitHub reports #254 MERGEABLE. That is an artifact of `main` being frozen at the 0.12.25
release, not evidence of compatibility.

```
gh/main                            = 61b79c4f  (= the PR's own base)
plan/f20-unified-audit-repair      = 873cc389
git rev-list --count gh/main..HEAD                          -> 1091
git rev-list --count gh/main..HEAD -- crates/wcore-sandbox/  ->   73
```

All of Phases 20–29 sit unmerged on the integration branch. Against that branch the PR
**conflicts in 3 of its 7 files**:

```
git merge-tree --write-tree --name-only HEAD pr-254
  CONFLICT (content): crates/wcore-sandbox/src/backends/appcontainer.rs
  CONFLICT (content): crates/wcore-tools/src/bash.rs
  CONFLICT (content): crates/wcore-tools/src/workspace_policy.rs
```

The conflict is worse than the count suggests. `appcontainer.rs` has been **split into a module
tree** upstream that does not exist at the PR's base:

```
crates/wcore-sandbox/src/backends/appcontainer/acl_lease.rs              (119 'lease' refs)
crates/wcore-sandbox/src/backends/appcontainer/acl_lease/storage.rs      (162)
crates/wcore-sandbox/src/backends/appcontainer/acl_lease/mutation_lock.rs
crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs   <- the PR's edits live here now
crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs
crates/wcore-sandbox/src/backends/appcontainer/windows_impl/handles.rs
```

Every hunk the PR applies to token creation, the attribute list and the cwd targets code that
has **moved to `windows_impl/process.rs`**. This cannot be conflict-resolved; it must be
re-authored. That is true even for the parts worth taking.

---

## 3. The three claims, graded independently

| # | Claim | Grade | Still live upstream? |
|---|---|---|---|
| C1a | drop `$HOME` from the Windows read allowlist | **DROP — superseded** | No — already fixed, better |
| C1b | grant `%TEMP%\wayland-scratch`, not all of `%TEMP%` | **TAKE** (re-authored) | **Yes — bug still present** |
| C2 | `SidsToDisable` → `Administrators` only (DLL-init) | **DROP — superseded** | No — already fixed, further |
| C3 | strip `\\?\` verbatim prefix from cwd | **TAKE** (re-authored) | **Yes — bug still present** |
| C4 | **Relaxed Sandbox Mode** | **REJECT as implemented** | n/a — new surface |
| C5 | Windows `BashTool::description()` quoting warning | TAKE WITH CHANGES | Yes — but see §3.5 |

### 3.1 Claim "DACL cost storm" — SPLIT: half superseded, half live and worth taking

The diagnosis is sound and, notably, the fix is a **net tightening** of the Windows allowlist,
not a loosening. Each allowlisted root is materialized as an inheritable ACE via
`SetNamedSecurityInfoW` per spawn and revoked after — genuinely O(subtree). The
`electron/electron#51761` blast-radius argument (a crash-leaked ACE on the profile root
bricking unrelated Chromium apps) is a real concern and well made.

**C1a is superseded.** Upstream no longer grants `$HOME` wholesale. `WorkspacePolicy::trusted_local`
now builds `readable_extra` from a curated capability set
(`crates/wcore-tools/src/workspace_policy.rs:183-190`):

```rust
let developer_capabilities = detect_developer_capabilities();
let mut readable_extra = developer_capabilities
    .iter()
    .flat_map(|capability| capability.read_only_roots.iter())
    .map(PathBuf::from)
    .collect::<Vec<_>>();
readable_extra.extend(trusted_config_and_certificate_reads());
```

This is **better than the PR's version**, which is a Windows-only `#[cfg(windows)] let
readable_extra = Vec::new();` fork. The upstream approach is capability-scoped and
cross-platform. Taking the PR's C1a would regress it. Drop it.

**C1b is live.** `scratch_dirs()` upstream is still the whole host temp tree
(`crates/wcore-tools/src/workspace_policy.rs:935-938`):

```rust
fn scratch_dirs() -> Vec<PathBuf> {
    let tmp = std::env::temp_dir();
    vec![canon(tmp)]
}
```

The PR's narrowing to a bounded `%TEMP%\wayland-scratch` subdir is still wanted, still
correct, and ships two real unit tests. **Take it, with one change** (§6, CR-3): the scratch
dir name is a fixed constant shared by *both* `trusted_local` (line 173) and `contained`
(line 242), so an untrusted contained session and a trusted session get write ACEs to the same
host directory. Key it per-trust or per-session. MEDIUM, non-blocking, but fix it while
re-authoring rather than after.

### 3.2 Claim "DLL-init bug" (C2) — correct diagnosis, already fixed upstream, and **it is not gated**

The diagnosis is right and was independently confirmed upstream. But upstream went **further**
than the PR — it passes `0/null` for `SidsToDisable` (disables nothing), where the PR still
disables `Administrators`
(`crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs:407-425`):

```
// No deny-only SIDs. An earlier revision marked BUILTIN\Administrators,
// BUILTIN\Users, and Authenticated Users as "for deny only" ... On a real
// AppContainer that marking is REDUNDANT and actively harmful: containment is
// intrinsic to the AppContainer package-SID access model ... The 2026-07-23
// hardware matrix confirmed identical reads exit 0 with the marking OFF and
// exit 1 with it ON ...
```

Two teams found the same root cause on the same day, and upstream's is backed by a hardware
matrix. **Drop C2 as redundant.**

One thing to carry forward: the PR's variant is arguably *tighter* than upstream's (it keeps
`Administrators` deny-only, upstream keeps nothing). If that tightening is wanted it is a
separate one-line change needing its own A/B evidence — not a reason to take this PR.

**Independently of supersession, C2 is the first place the PR's safety claim breaks.** The
change is unconditional — it is **not** behind the relaxed-mode flag — so it alters the token
of every `Contained` (untrusted/remote) child too. The PR text says contained sessions are
"completely untouched". That is false. The whole panel flagged this (§7).

### 3.3 Claim "cwd bug" (C3) — correct, live, take it

`cmd.exe` treating a `\\?\`-prefixed cwd as UNC and silently falling back to `C:\Windows` is a
real, well-known Windows behavior, and the fix names the identical filesystem object (the ACL
grant is on the object, not the spelling), so isolation is unaffected. It ships a three-case
unit test including the verbatim-UNC negative.

The bug **is still present upstream** — `windows_impl/process.rs:387-395` passes the path
through unmodified:

```rust
Some(widen_os(p.as_os_str()))
```

**Take it.** Cleanest single change in the PR.

*(One objection I considered and am dropping as unsubstantiated: that stripping `\\?\`
reintroduces the MAX_PATH limit. A Win32 process working directory is MAX_PATH-limited
regardless of the spelling passed to `lpCurrentDirectory`, so this is not a regression. Recorded
so it is not re-raised.)*

### 3.4 Claim "Relaxed Sandbox Mode" (C4) — REJECT as implemented. See §4.

### 3.5 C5 — `BashTool::description()` (bonus, not in the headline claims)

The `cmd /C` nested-double-quote mangling is real; I hit the identical class of bug driving
this very review over SSH and had to switch to `-EncodedCommand`. Putting the warning in the
LLM-facing description is the right call — the agent otherwise retries a shape that cannot work.

**Change requested:** the patch duplicates the *entire* ~20-line description string under
`#[cfg(windows)]` / `#[cfg(not(windows))]`. The two copies will drift. Compose a shared base
string with a cfg'd Windows suffix instead. LOW.

---

## 4. Relaxed Sandbox Mode — the security verdict

**Verdict: this is a hole, not a sound ergonomics tradeoff — as implemented.** Three defects,
each independently disqualifying. The idea may be defensible; this implementation is not.

### 4.1 D-1 — "trusted_local only" exists **only in comments**. It is not enforced anywhere. (CRITICAL)

The resolver consults nothing but a process-global env var / config value:

```rust
// crates/wcore-sandbox/src/backends/appcontainer.rs:484
fn relaxed_windows_sandbox() -> bool {
    crate::windows_relaxed_sandbox_enabled()   // env var, else global config. That is all.
}
```

`trusted_local` appears in that file at lines 458, 466, 472, 476, 1413, 1768 — **all six are
doc comments.** There is no `WorkspaceTrust` check anywhere in the spawn path.

It is not merely unimplemented — **it is not implementable as written**:

- `SandboxManifest` (what `execute()` receives) has no trust field at all. Its fields are
  `fs_read_allow`, `fs_write_allow`, `fs_read_deny`, `network`, `syscall_policy`, `timeout`,
  `max_memory_bytes`, `max_cpu_secs`, `env`, `image` (`crates/wcore-sandbox/src/manifest.rs:46-86`).
  The backend cannot see the session's trust level even in principle.
- Backend selection (`default_for_platform()`, `crates/wcore-sandbox/src/lib.rs:698`) is
  process-global and trust-agnostic. `Contained` sessions on Windows use the *same*
  `AppContainerBackend`.

So once the flag is on, **every** AppContainer spawn — including `Contained` untrusted/remote
sessions — loses the AppContainer capability (`attr_count = 1`, `SECURITY_CAPABILITIES` skipped),
loses the forced Low integrity level, loses the post-spawn Low-IL assertion, and un-blocks
PowerShell/`bash` via a host-`PATH` scan.

The PR's own new live test only asserts the positive direction (the toolchain runs). There is
**no** test asserting contained sessions keep the AppContainer — consistent with there being no
mechanism that could make one pass.

### 4.2 D-2 — `fs_read_deny` silently becomes a no-op: a control that reports itself active while being inactive (CRITICAL under Phase 28's own rubric)

Both allow and deny ACEs are keyed to the AppContainer **package SID**:

```rust
// appcontainer.rs:1466,1482
grant_appcontainer_dacl(&manifest.fs_read_allow, sid_ptr, ACL_READ_MASK)?;
deny_appcontainer_dacl(&manifest.fs_read_deny, sid_ptr)
```

In relaxed mode the child never receives that SID. A deny ACE naming a SID absent from the
child's token matches nothing. The code still *runs* the grant/deny path, still pays its cost,
still logs `"granted AppContainer DACL access"` — and enforces nothing. `compute_secret_deny()`
(the SSH-key / cloud-credential / `.env` denial set) is silently inert.

This is the literal CRITICAL row of Phase 28's own severity rubric
(`.planning/phases/28-native-cross-platform-certification/28-01-CERTIFICATION-CONTRACT.md:145`):

> | CRITICAL | The candidate contradicts a Success Criterion in a way that would make the signed
> receipt false, **or a security control reports itself active while being inactive.** |

Under **Amendment A2** that disposition can only be FIXED or DISPROVED — never accepted, never
deferred. Merging C4 would inject a CRITICAL finding into Phase 28 immediately before 28-04
builds the certification finding-ledger.

### 4.3 D-3 — the flags are reachable from an **untrusted project config** (GHSA-8r7g violation) (HIGH/CRITICAL)

`.wayland-core.toml` is resolved from the project directory —
`project_config_selection()`, `crates/wcore-config/src/config.rs:3140-3143` — i.e. the file that
travels with a cloned repo. The repo's own invariant is explicit
(`config.rs:4003-4006`):

> *"`auto_approve` and `allow_no_sandbox` are privilege-granting flags. A project config
> (untrusted — travels with a cloned repo) must not be able to raise them beyond the user's
> global posture. Clamp both tighten-only."*

`allow_no_sandbox` is clamped. `auto_approve` is clamped. `allow_list` is clamped. The PR's two
new flags are **not** — they use plain `project.or(global)` in the same function:

```rust
windows_relaxed_sandbox: project.tools.windows_relaxed_sandbox.or(global.tools.windows_relaxed_sandbox),
windows_allow_admin:     project.tools.windows_allow_admin.or(global.tools.windows_allow_admin),
```

**Attack:** a hostile repo ships `.wayland-core.toml` containing
`[tools] windows_relaxed_sandbox = true` / `windows_allow_admin = true`. Victim clones, opens it
in Wayland Core on Windows. The AppContainer is gone, Low IL is gone, the secret-denial ACEs are
inert, PowerShell/bash are unblocked — and if the host happens to be elevated, the child runs
with the unrestricted admin token. No prompt distinguishes this from normal operation.

Note the contrast with the existing escape hatch: `WAYLAND_SANDBOX=none` requires **two**
signals (`WAYLAND_ALLOW_NO_SANDBOX=1` as well), is clamped tighten-only, and warns loudly.
`windows_relaxed_sandbox` is one unclamped project-config key. The categorical difference is
real, so "we already have `allow_no_sandbox`, this adds nothing" does not rescue it.

### 4.4 Interaction with `cba6d2b6` ("reject remote sandbox bypass")

`cba6d2b6` narrowed the **JSON-stream protocol** vocabulary so a remote peer cannot claim
sandbox authority over the wire — *"Dangerous mode remains a local lease-only capability"*
(`crates/wcore-protocol/src/commands.rs`). C4 does not touch that file and does not reopen the
wire path.

But it **routes around the intent by a different door.** `cba6d2b6` establishes that sandbox
authority must not arrive from outside the local operator's own decision. C4 lets sandbox
authority arrive from repo content — a channel `cba6d2b6` never had to consider because no
config key previously carried sandbox authority. The wire is still shut; a new unclamped
side-channel opens next to it. Same intent, defeated.

### 4.5 What is genuinely valuable in C4, and must not be discarded

The contributor's empirical claim deserves to survive the rejection:

> *every Windows primitive that scopes writes by identity (the AppContainer SID, Low integrity,
> a restricted token) also blocks the reads these tools need at startup. There is no allowlist,
> amortized or not, that gets both.*

They report `git init`, `npm`, msys `ls`/`pwd` and PowerShell still failing **after** C1–C3, and
say they measured an amortized full-`$HOME` grant costing 66s and still not fixing it. If that
holds, **the Windows Bash tool cannot run a normal dev toolchain under the current sandbox at
all** — a product-level fact this program should know independently of this PR. Rejecting C4
without opening that as a tracked issue would throw away the most valuable thing here.

I did **not** independently reproduce the `git init`/`npm` failures (§5, NOT MEASURED).

---

## 5. Build and test evidence

All figures below are from runs I executed and read. Nothing is inferred.

### Linux — `hetzner-dsm`, worktree `/root/wayland-core254` @ `0c995cec` (PR head)

```
cargo test -p wcore-sandbox -p wcore-tools -p wcore-config --lib
  wcore-config   444 passed; 0 failed
  wcore-sandbox   42 passed; 0 failed
  wcore-tools    959 passed; 0 failed; 3 ignored
  EXIT=0
```
**1445 passed / 0 failed.** Caveat stated plainly: **this proves almost nothing about this PR.**
Essentially all of it is `#[cfg(windows)]` — hence only 42 sandbox tests here vs 78 on Windows.

### Windows — `SeanD@seandesktop`, worktree `C:\ferrox-core254` @ `0c995cec` (PR head)

```
cargo test -p wcore-sandbox -p wcore-tools -p wcore-config --lib
  wcore-config   444 passed; 0 failed
  wcore-sandbox   78 passed; 0 failed; 6 ignored   <- includes the new strip_verbatim_disk_prefix test
  wcore-tools    932 passed; 3 failed; 2 ignored
  EXIT=101
```

The 3 failures are `transcription_tools::tests::oversized_payload_rejected`,
`vision_tools::tests::hermetic_tempdir_fixture`, `vision_tools::tests::oversized_payload_rejected`
— vision/transcription tooling, no plausible connection to the sandbox changes.
**Baseline status: see §5.1.** Contention note: another lane's `wlWinRequeueKr01` task was
Running throughout; I ran under my own task (`wlCore254Build`) and did not disturb it.

**The PR compiles clean and its own tests pass on Windows, which is the platform that matters
here.** `wcore-sandbox` 78/78 is the meaningful number.

### 5.1 Windows baseline at `61b79c4f` — the PR turns a GREEN Windows suite RED

**This is the one place my first measurement was wrong, and correcting it changed a finding.**
My initial comparison ran the baseline *filtered to 3 tests* against a *full-suite* head run —
not a valid comparison. Redone properly, all four cells:

| Commit | Scope | Result |
|---|---|---|
| `61b79c4f` (base) | **full** `wcore-tools --lib` | **932 passed; 0 failed; 2 ignored — EXIT=0** |
| `0c995cec` (head) | **full** `wcore-tools --lib` | **932 passed; 3 failed; 2 ignored — EXIT=101** |
| `61b79c4f` (base) | the 3 tests, isolated | 3 passed; 0 failed |
| `0c995cec` (head) | the 3 tests, isolated | 3 passed; 0 failed |

**The base full suite is green; the head full suite is red. The same 3 tests pass in isolation
at both commits.** So #254 as submitted turns the Windows `wcore-tools` suite red, and its
checklist claim *"Unit tests pass ... no regressions"* does not hold on Windows.

**Root cause — and it is NOT the contributor's production code.** All three failures are the
same assertion:

```
panicked at crates\wcore-tools\src\vision_tools.rs:679:9:
got error result: {"error":"website policy present but could not be evaluated","success":false}
```

`website_policy.rs:670-700` mutates the **process-global** `WAYLAND_HOME` env var and resets a
process-global policy cache, justified by this comment:

```rust
// SAFETY: `#[serial_test::serial]` serializes every env-mutating test
// in this binary, so this mutation cannot race another.
unsafe { std::env::set_var("WAYLAND_HOME", dir.path()) };
```

**That justification is false.** `#[serial_test::serial]` serializes only against *other tests
also annotated `#[serial]`*. The three failing tests are plain `#[test]`
(`vision_tools.rs:661,915`, `transcription_tools.rs:791`), so they run in parallel with it and
observe the mutated global policy mid-flight.

The PR adds exactly 3 new `wcore-tools` tests, which perturbs the parallel schedule enough to
make a **pre-existing latent race** manifest. Attribution, stated precisely: **the PR exposes
this defect; it does not cause it.** It is nonetheless a live CI blocker for anyone landing
this work, and a real finding in its own right.

> **Spun out as a separate backlog item (MEDIUM, test-only, not #254's fault):**
> `wcore-tools` test suite is not hermetic — a `#[serial]`-annotated test mutates global env +
> a global cache while unannotated tests read them concurrently. Fix by annotating the readers
> `#[serial]` too, or by threading `WAYLAND_HOME` as a parameter instead of an env var. This
> should be fixed regardless of what happens to #254.

**Determinism re-run: NOT COMPLETED — reported as such, not as a pass or a fail.** A repeat
full-suite run at head aborted with an intermittent
`STATUS_ACCESS_VIOLATION (0xc0000005)` in the `wcore-config` test binary, which had passed
444/444 in the first head run. An intermittent access violation is itself consistent with the
same `std::env::set_var` data race (concurrent `setenv`/`getenv` is genuine UB), but I did not
prove that, and `wcore-config` is barely touched by this PR. **I could not establish whether the
3-failure result is deterministic.** It reproduced once; that is all I can claim.

### NOT MEASURED — stated as such, not rendered as a pass

| Leg | Why |
|---|---|
| `live_relaxed_windows.rs` (the PR's own live test) | Requires `WAYLAND_SANDBOX_LIVE_WINDOWS=1` and a real interactive AppContainer box. Not run. **Note it is a self-passing gate when that var is unset** (`if !live() { return; }`), so the PR's "unit tests pass" checkbox does not cover it. |
| The `git init` / `npm` / msys failures under strict AppContainer | Not independently reproduced. The contributor's core C4 premise is **unverified**, neither confirmed nor refuted. |
| The 35s → 20ms and 66s DACL measurements | Not reproduced. Mechanism is sound; the numbers are the contributor's. |
| Behavior of the PR merged onto the integration branch | Not built — it does not merge (§2), so there is nothing to measure without re-authoring. |

---

## 6. If Sean wants it landed: the exact change list

Re-authored by a maintainer against `plan/f20-unified-audit-repair`, **not** a rebase of #254.

| ID | Change | Sev |
|---|---|---|
| **CR-1** | Take C3 (`\\?\` cwd strip) into `appcontainer/windows_impl/process.rs:387-395`, with the PR's 3-case unit test. | — |
| **CR-2** | Take C1b (`%TEMP%\wayland-scratch`) into `workspace_policy.rs:935`, with the PR's 2 unit tests. | — |
| **CR-3** | While doing CR-2: key the scratch dir per-trust/per-session. Today one fixed name is shared by `trusted_local` (line 173) and `contained` (line 242) — a cross-trust shared writable dir. | MEDIUM |
| **CR-6** | Independently of #254: fix `wcore-tools` test hermeticity (§5.1). Without it the Windows suite is red whenever scheduling shifts. | MEDIUM |
| **CR-4** | Drop C1a and C2 — superseded (§3.1, §3.2). | — |
| **CR-5** | Take C5 as a shared base string + cfg'd Windows suffix, not two full copies. | LOW |

**If C4 is ever revisited, the minimum bar (all four, not a subset):**

1. Thread `WorkspaceTrust` into `SandboxManifest` (or resolve before backend dispatch) and
   enforce `Trusted` **at the spawn site**. A doc comment is not a control.
2. Clamp both config keys tighten-only per GHSA-8r7g — a project config may only ever *disable*
   relaxed mode.
3. Fail closed when policy cannot be enforced: if `fs_read_deny` is non-empty and there is no
   package SID to key it to, **refuse the spawn**. Never run a control that reports itself
   applied while enforcing nothing.
4. A negative test proving a `Contained` session keeps AppContainer + Low IL + the post-spawn
   assertion *with the flag on and a hostile project config present.*

---

## 7. Cross-audit panel

### 7.0 Panel integrity — audited after a contamination warning, then re-run

Mid-review the coordinator reported that a concurrent lane had measured **prompt-path clobbering
causing two panel members to fluently answer a different lane's question** — and that the
question they answered was likely a PR-review question, i.e. plausibly this one. I treated the
round-1 panel as suspect and verified it rather than trusting it.

**Round 1 audit (all three legs passed):**

| Check | Result |
|---|---|
| Prompt path session-scoped? | Yes — under this session's own UUID-keyed scratchpad, not a shared filename |
| Response answers *my* question? | Yes, by content: all 3 name `windows_relaxed_sandbox`, `windows_allow_admin`, `fs_read_deny`, `GHSA-8r7g`, `AppContainer` |
| Contamination markers (criteria gaps, phase verdicts, lane costs) | **0 in all three** |
| Byte counts non-trivial? | 22,296 / 20,470 / 6,004 — none dropped, none truncated |

**The echo hazard was real and I dodged it by luck of method, so it is worth recording.**
`codex.txt:48` contains the prompt's own echoed template line:

```
codex.txt:48   PANEL_POSITION=<MERGE-AS-IS|MERGE-WITH-CHANGES|REJECT|SPLIT-AND-TAKE-PART>
codex.txt:252  PANEL_POSITION=SPLIT-AND-TAKE-PART      <- real answer
codex.txt:336  PANEL_POSITION=SPLIT-AND-TAKE-PART      <- codex repeating its final block
```

An anchored first-match grep would have captured **line 48 — my own question — as codex's
vote.** I extracted unanchored + last-match, which took line 336 correctly. I also caught kimi
mid-write at 4,071 bytes and re-counted at 6,004 before reading its position.

**Round 2 — re-run anyway, per the coordinator's instruction**, on a lane-unique path
(`scratchpad/core-254-panel/core-254-Q.txt`, md5 `dcef4b98e92cfc02b23c96f7f69bc0ec`) with
**stdin redirected from `/dev/null`** on every leg (addressing the reported `codex exec`
inherited-stdin defect):

| Leg | R1 bytes | R2 bytes | R1 position | R2 position |
|---|---|---|---|---|
| codex | 22,296 | 13,669 | SPLIT-AND-TAKE-PART | **SPLIT-AND-TAKE-PART** |
| gemini | 20,470 | 20,593 | SPLIT-AND-TAKE-PART | **SPLIT-AND-TAKE-PART** |
| kimi | 6,004 | 8,484 | SPLIT-AND-TAKE-PART | **SPLIT-AND-TAKE-PART** |

All six captures are non-trivial, on-topic (`GHSA-8r7g` and `fs_read_deny` present in every
round-2 response; kimi r2 shows 17× `AppContainer`, 4× `package SID`), and unanimous across two
independent rounds. Severity dissent also held: kimi r2 again *"Severity: High (CVSS-ish 8)"*.

**Conclusion: this panel is uncontaminated and I can prove it two ways** — by content
fingerprint and by an independent re-run on an isolated path. It is safe to count. Had I been
unable to show that, I would have reported it as unusable rather than counted it.

### 7.1 Result

Four legs. Byte counts captured to catch a silently dropped vote; positions extracted
unanchored, last-match.

| Leg | Bytes | Position |
|---|---|---|
| `codex exec -m gpt-5.6-sol` | 22,296 | SPLIT-AND-TAKE-PART |
| `gemini -m gemini-3.1-pro-preview` | 20,470 | SPLIT-AND-TAKE-PART |
| `kimi` (K3) | 6,004 | SPLIT-AND-TAKE-PART |
| internal adversarial | — | SPLIT-AND-TAKE-PART, **with two corrections to the other three** |

**Split: 4/4 on SPLIT-AND-TAKE-PART.** Unanimous on: C1 accept, C2 accept-with-changes, C3
accept, C4 hold/reject, and that the "contained untouched" claim is false.

**Severity dissent on D-3, recorded rather than averaged:**
- gemini — **Critical**: *"Yes, this is a **Critical** vulnerability... An attacker can commit a
  malicious `.wayland-core.toml`... the untrusted config will silently bypass the sandbox and
  grant unrestricted (potentially Administrator) RCE."*
- codex — **High**: *"**Severity: High**, with potentially critical impact when Wayland Core is
  elevated or tool execution proceeds without meaningful per-command intervention."*
- kimi — **High, Critical for one flag**: *"**Severity: High** (I'd argue Critical for
  `windows_allow_admin` specifically — it's a one-line config-to-admin-code-execution primitive
  delivered by `git clone`)."*

I record it as **CRITICAL**, siding with the minority-by-count but stronger-by-evidence
position, on a ground none of the three legs had: Phase 28's own rubric assigns CRITICAL to "a
security control reports itself active while being inactive", which D-2 satisfies literally
(§4.2). That is a repo-specific authority, not a judgement call.

**Dissent from codex on C2, verbatim, and why I did not adopt it:**
> *"C2 — ACCEPT-WITH-CHANGES: Enabling `BUILTIN\Users` may be necessary for DLL loading, but
> enabling `Authenticated Users` is unexplained and the resulting Contained-session widening
> requires explicit threat analysis and negative security tests."*

Sound in isolation, and it would be my position too if the PR were being judged against `main`.
It is moot in fact: upstream already disables **no** SIDs at all (`0/null`), backed by a
2026-07-23 hardware matrix (§3.2) — strictly more permissive than what codex objects to. The
panel was deliberately given the PR-vs-`main` framing and was not told the integration-branch
state, so this is a limitation of my question, not an error by codex. **Flagged for Sean:** if
he shares codex's "minimum necessary widening" instinct, the thing to re-examine is the
*upstream* `0/null` decision, not this PR.

**Internal adversarial pass — two corrections to the emerging consensus:**
1. *Against "take C1".* All three legs said take C1 whole. Wrong for this tree: C1a would
   **regress** upstream's curated capability allowlist back to a Windows-only `Vec::new()` fork.
   Only C1b survives. The legs could not know this.
2. *Against "just reject C4".* Rejecting the implementation while discarding the finding leaves
   a possibly-unusable Windows sandbox undocumented (§4.5). The rejection must ship with a
   tracked issue.
3. *One objection I raised and then killed*: that C3 reintroduces MAX_PATH. It does not — a
   Win32 process cwd is MAX_PATH-limited either way. Recorded so it is not re-raised.

---

## 8. `F-28-02-002` — interaction

**Verdict: LEFT UNTOUCHED. It neither closes nor worsens it — but it creates pressure toward a
dangerous workaround.**

- The PR **does not touch** the probe or lease path. Diffing the whole of `crates/wcore-sandbox/`
  between base and PR head yields exactly one match for `is_available|probe|lease` — a doc
  comment line. No logic.
- It **cannot** address it: `F-28-02-002` is a stale-lease wedge in the `acl_lease` subsystem,
  and that subsystem **does not exist at the PR's base**. At `61b79c4f` the probe cache is an
  in-process `Instant` TTL (`NEGATIVE_PROBE_TTL = 30s`); the durable on-disk lease is upstream
  work the PR has never seen.
- **Weak positive:** C1b reduces the number and size of ACL'd roots, which shrinks the
  grant/revoke window in which a crash can strand a lease. That lowers the *incidence* of the
  wedge without changing its *mechanism*. Do not score it as a fix.
- **The interaction that actually matters, and it is adverse.** A user hitting `F-28-02-002` sees
  the product refuse to execute with a message that reads like a platform limitation. If C4
  ships, the obvious remedy circulating in issue threads becomes
  `[tools] windows_relaxed_sandbox = true` — which "fixes" the DoS by removing the control that
  is wedged. **C4 converts a fail-closed denial of service into a standing incentive to disable
  the sandbox.** That is a worse outcome than the DoS, and it is exactly the fail-closed→fail-open
  inversion `F-28-02-002` was scored HIGH-not-CRITICAL for avoiding.

---

## 9. Risk statement

**If Sean follows this recommendation** (split, take CR-1/CR-2, reject C4):
- Cost: maintainer time to re-author two small fixes (~half a day incl. a Windows test pass).
- **Contributor-relations risk, and it is the real one.** `frankforges` did careful work, offered
  the split themselves (*"Happy to split #4 into its own PR"*), and disclosed AI assistance
  honestly. A rejection that reads as dismissive loses a capable Windows contributor — and
  Windows is where this program is thinnest. The response should lead with what was right, name
  the two fixes being taken, and be specific that C4's *problem statement* is being kept.
- Residual: Windows users still cannot run the full toolchain under the sandbox. Unchanged from
  today, but now documented instead of unknown.

**If Sean does not** (merges #254 as-is):
- A repo-resident `.wayland-core.toml` becomes a one-key sandbox-disable, and a two-key
  admin-escalation primitive, on Windows — delivered by `git clone`, against an explicit
  invariant the codebase enforces three times in the same function.
- `fs_read_deny` silently stops enforcing while still reporting applied — a CRITICAL by Phase
  28's own rubric, landing weeks before 28-04 signs a certification receipt.
- The merge does not apply cleanly anyway (§2) and would regress upstream's curated `$HOME`
  allowlist back to a Windows-only stub.
- Windows CI goes red on `wcore-tools` the moment it lands (§5.1) — not the contributor's fault,
  but it blocks the merge in practice until CR-6 is done.

**Confidence.** High on everything derived from reading code (§3, §4, §8) — those are file:line
facts, re-checkable in minutes. Medium on §4.5, which rests on contributor measurements I did
not reproduce. The recommendation does not depend on §4.5 being true: if it is false, C4 is
unnecessary as well as unsafe; if it is true, C4 is necessary but still unsafe as written, and
the tracked issue is what carries it.

---

## 10. Provenance

| | |
|---|---|
| Branch | `lane/core-254` |
| Base | `plan/f20-unified-audit-repair` @ `873cc389` |
| PR head reviewed | `0c995cece24d888c9acef99970ee301d61c2ef2b` |
| Linux evidence | `hetzner-dsm:/root/core254-linux.log`, worktree `/root/wayland-core254` |
| Windows evidence | `seandesktop:C:\ferrox-core254\win.log`, baseline `C:\ferrox-core254base\base.log` |
| Panel transcripts R1 | scratchpad `panel/{codex,gemini,kimi}.txt`, question `panel/Q.txt` |
| Panel transcripts R2 | scratchpad `core-254-panel/core-254-{codex,gemini,kimi}-r2.txt`, question `core-254-panel/core-254-Q.txt` (md5 `dcef4b98e92cfc02b23c96f7f69bc0ec`) |

**Shared-host courtesy:** Windows work ran under my own scheduled tasks (`wlCore254Build`,
`wlCore254Base`, `wlCore254Iso`, `wlCore254Full`). The other lane's `wlWinRequeueKr01` was only
ever *queried*, never started, stopped or modified; it completed on its own. Worktrees
`C:\ferrox-core254`, `C:\ferrox-core254base` and `/root/wayland-core254` are left in place for
re-checking and can be removed once this is decided. `C:\ferrox-win-23B04` and the protected
hetzner worktrees were not touched.

**Not done, by design:** no merge, no PR comment, no review submission, no close, no push to the
contributor's branch, no edit to any file under `crates/`. This lane read the PR and tested it;
every GitHub action on #254 remains Sean's.
