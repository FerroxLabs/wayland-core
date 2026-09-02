# wayland-core — MASTER PLAN

The single durable ledger-driven plan for the core lane. Written to be read by a
session with **no memory of any prior session**. It stands alone.

Last updated 2026-08-29 against baseline **v0.13.10 = `cfa89a9c`** (`origin/main`,
`Cargo.toml version = "0.13.10"`; the `v0.13.10` tag is an annotated tag over that
same commit — `git rev-list --count v0.13.10..origin/main` is 0).

---

## 1. How to use this document

`.planning/ledger/` is the **authoritative per-criterion state**. One file per open
issue, on **both** trackers, named `<repo-slug>-<number>.md` (`wayland-1088.md`,
`wayland-core-335.md`). Each criterion carries `id`, `text`, `state`
(`met` | `not-met` | `blocked` | `superseded`), `owner`
(`core` | `desktop` | `flux` | `maintainer` | `reporter`), and — for `met` — one
machine-resolvable `evidence` token (`test:`, `symbol:`, `file:`, `commit:`).

**This file is the ordered plan over that ledger. It does not restate it.** Where a
number, a state, or a piece of evidence disagrees between the two, the ledger wins.

```bash
just ledger-check          # offline arm: parses every file, resolves every `met` evidence token.
                           # Chained into `check-all` (justfile:247). Fails closed.
just ledger-check-live     # adds tracker COVERAGE across BOTH repos. Needs gh. Runs at release.
```

**To see what is outstanding right now, in one line:**

```bash
grep -c 'state: not-met' .planning/ledger/*.md | grep -v ':0$'
```

or for the whole picture, `python3 scripts/check-criteria-ledger.py --offline`, whose
first line is the census. At the time of writing:

```
ledger files: 52   criteria: 198   met 71 / not-met 110 / blocked 15 / superseded 2
```

`--offline` is **not a pass for coverage** and says so in its own output. An entire
tracker can be missing and the offline run is still green.

---

## 2. Scope

52 ledger files: **43 open**, 9 closed (wayland 1134, 1161, 1162, 1163, 1166, 1168,
1170, 1171, 1173). The 43 open decompose exactly:

| Set | Count | Numbers |
|---|---|---|
| `FerroxLabs/wayland-core` — the whole tracker, all open | **17** | 113, 238, 244, 253, 314, 322, 323, 324, 325, 335, 336, 337, 338, 339, 340, 342, 350 |
| `FerroxLabs/wayland` — PARTIAL: half shipped in v0.13.10, half did not | **12** | 388, 434, 559, 908, 934, 998, 1088, 1150, 1151, 1155, 1156, 1172 |
| `FerroxLabs/wayland` — filed *during* the v0.13.10 cycle, unworked | **9** | 1174, 1175, 1176, 1177, 1178, 1179, 1180, 1181, 1182 |
| `FerroxLabs/wayland` — older, still open | **5** | 174, 305, 863, 1164, 1165 |

Plus infrastructure items that are not issues: the `LEDGER_ISSUES_TOKEN` secret
(§8), the four orphaned lane branches (#1181), and four in-flight lanes (§4.0).

**Outstanding criteria by owner** (not-met + blocked = 125 of 198):

| Owner | not-met | blocked | total outstanding |
|---|---|---|---|
| core | 109 | 0 | **109** |
| maintainer | 1 | 6 | **7** |
| flux | 0 | 5 | **5** |
| desktop | 0 | 4 | **4** |

The gate forbids a `blocked` criterion owned by `core` — core cannot block on itself.

> **Line numbers in this document were verified against tree
> `77954d92a233ab7133a8746fd4e56b6b2bb70dba` (`cfa89a9c`). Re-verify every
> `file:line` against the tree sha you are actually holding before trusting it.**
> Use `grep -n` on the symbol, not the number.

---

## 3. Why this was missed

Issues live on **two** trackers: `FerroxLabs/wayland` (core work carries label
`area:core`) and `FerroxLabs/wayland-core` (no label filter — the whole tracker is
ours). The work queue was filtered by label against `FerroxLabs/wayland` only, so all
17 `wayland-core` issues returned no results and "no results" was read as "no work" —
for an entire release cycle.

`scripts/check-criteria-ledger.py` now hard-codes both trackers in source
(`TRACKERS = [("FerroxLabs/wayland", "area:core"), ("FerroxLabs/wayland-core", None)]`)
and fails closed on exactly this miss: an open issue in **either** tracker with no
ledger file is an ORPHAN and stops the release; a tracker query that reaches zero
issues is a FAIL, not a green; and a `gh issue list` that returns exactly `GH_LIMIT`
rows is treated as truncated and fatal (this already fired once — at limit 500 the
`wayland` query truncated and reported two open issues as ORPHANs).

---

## 4. Ordered work plan

### 4.0 In flight — record the state, do not re-plan

| Lane | Tip | State | Covers |
|---|---|---|---|
| `lane/atref` | `6d130a62` | **DONE, gated, pushed to origin, UNMERGED** | core#339, core#335, core#323 as one commit; refutes core#322 |
| `lane/finish-a` | `3281c2c6` | IN PROGRESS — **local to hetzner, NOT pushed to origin** | wayland#934 boundary probe, #1150 web_fetch truncation, #908 sub-symptoms |
| `lane/finish-b` | `eb2f2635` | IN PROGRESS — **local to hetzner, NOT pushed to origin** | wayland#1155 residuals, #1156 test-supervisor half, #1172 buffer arithmetic (#1179) |
| `lane/ledger` | `f79af6aa` | DONE, pushed, UNMERGED | the ledger + its gate; needs `LEDGER_ISSUES_TOKEN` (§8) |

All four have `origin/main` as an ancestor (verified with `git merge-base --is-ancestor`).
**`lane/finish-a` and `lane/finish-b` exist only on the hetzner box.** A machine loss
loses that work. Push them.

### 4.1 Lane 0 — no code, do first

| Step | Action | Why first |
|---|---|---|
| 0.1 | Grep retained Windows CI logs for `TRY n FAIL` on the inv2 test names. `grade-retry-flakes.sh:87` already parses that marker, and `inv2_round5_adversarial_test.rs:629-636` / `:1023-1033` already run **ungated** on Windows. | May deliver core#342's Windows number for free and save ~150 runs on the single Windows box. |
| 0.2 | Add a `retries = 0` override for `inv2_round5_adversarial_test` in `.config/nextest.toml`. `grep -c inv2 .config/nextest.toml` is currently **0**, so it inherits `[profile.ci] retries = 2` (`nextest.toml:563-572`) — a test that fails twice and passes once reports as PASSED in the run conclusion. | Without this, every #342 measurement is biased clean. |
| 0.3 | Declare the Windows merge freeze window; do not open it until 0.1 has been read. | §8, maintainer call. |

### 4.2 Ordering that is load-bearing

1. **core#350 is the FIRST code change of all of them.** It runs on the *required*
   `CI (Array)` leg on every PR, so every other PR in this plan must pass it. Fixing
   it first is the cheapest de-risking of the entire merge path.
2. **core#350 fixed and core#324 dispositioned BEFORE core#325's honest gate lands.**
   A draft ordering that made #350/#324 depend on #325 is **spurious and inverted**:
   #325's gate cannot close while #324 (in `windows-live-acceptance`) and #350 (in
   `windows-soak`) still flake, so landing #325 alone converts a false auto-close into
   a permanently-open tracker plus a daily red report — alert fatigue, which is the
   same defect with the sign flipped. Alternative if the flakes are not closed first:
   land #325 with a **named carve-out with an expiry date** for exactly those two.
3. **core#340 slice 1 before slice 2.** Slice 1 is genuinely independent.
4. **Lane Z runs LAST, as one commit, with one contract-corpus regen.** See 4.4.

### 4.3 Fixes that MUST land as one change — splitting is unsafe

| Bundle | Why splitting is unsafe |
|---|---|
| **wayland#1174 + wayland#1175** | Load-bearing on each other. Fixing #1175 without threading `McpServerConfig` into `server_configs` would make the allow-all read at `crates/wcore-mcp/src/tool_proxy.rs:447` **go live** and restore a server's full tool set on `list_changed`. Today that read is unreachable — config-declared servers are present in `server_configs` with their real `allowed_tools` (`bootstrap.rs:1853`), and plugin-declared servers are constructed with `allowed_tools: None` at boot too (`plugins/mcp_delivery.rs:57/69/81`), so a refresh reproduces boot exactly. Register the manager without also carrying the config and you have opened a privilege escalation. **That must be one change.** |
| **core#335 + core#339** | #339's canonicalization must not land while `rel_to_root`'s `None` branch still means "read anyway" — that branch converts a canonicalization bug into a **silent gitignore downgrade** rather than a loud failure. (Already satisfied: `lane/atref` @ `6d130a62` lands both in one commit.) |
| **core#338 layer 1 + layer 2** | Layer 1 (`GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS`, `SSH_ASKPASS`) suppresses git's *own* prompt, so the acceptance arm's red signal ("nothing written to the pty") goes green while the `credential.helper` → `/dev/tty` path stays open. Landing layer 1 alone ships a test certifying a property the code does not have. |
| **core#338 layer 2 + its teardown decision** | `setsid`/`process_group(0)` detaches git but does not close the pipes or extend the kill (`quarantine.rs:29-44` `DRAIN_GRACE`; timeout path ~`:325` kills the direct child only). Split, it converts a bounded wedge into an unreaped detached tree. |
| **core#314 D-1 + its missing test** | The test *is* the deliverable; the receipt move is one line. |
| **core#340 slice 2b + core#314 D-2** | Both need a **protocol frame, not a log line**. Decide together, land in the single Lane Z regen. |

### 4.4 Lanes that can run in parallel

| Lane | Contents | Blocked by |
|---|---|---|
| **A** | core#350 | nothing — go first |
| **B** | core#339 + core#335 (`lane/atref`, done) | merge review only |
| **C** | core#337, core#336 | nothing |
| **D** | core#253 buried Telegram defect (split out as its own bug) | nothing |
| **E.1** | core#340 slice 1 (runner/argv parsing) | nothing |
| **E.2/E.3** | core#340 slices 2/3 | Q3+Q4 decision; then Lane Z |
| **F** | core#325 honest gate | Lane A + #324 disposition |
| **G** | core#314 D-1 | Lane Z (SOURCE_INPUTS) |
| **H** | core#238 Linux half | Q5 decision |
| **M** | wayland#1174 + #1175 (one change) | nothing |
| **Z** | **serialized, LAST, one commit, one corpus regen** | everything above |
| **W** | **the Windows box — a serialized resource, one job at a time** | Lane 0.1 |

**Lane Z exists because of a real collision.** `spec.rs:1314-1355` lists
`wcore-cli/src/main.rs`, `wcore-mcp/src/transport/stdio.rs`,
`wcore-plugin-subprocess/src/mcp_bridge.rs`, `commands.rs`, `events.rs` as
`SOURCE_INPUTS`, and `generate.rs:1311-1320` hashes **raw file bytes** — so a
doc-comment-only edit reddens `desktop_contract_corpus.rs:199-207` on
`CI (Array)` **and** `CI (linux-containerized)` **and** `CI (macos-latest)`
simultaneously (`ci.yml:653-660`). Two unrelated PRs have already lost a night to
this. Four fixes collide there: #340 slice 2a (`stdio.rs:509-512`), #340 slice 3,
#314 D-1 (`main.rs:4196/4222/4276/4302`), #314 D-2 (`commands.rs`/`events.rs`).
Verified **not** affected: #340 slice 1, #339, #335, #338, #253, #342, #336, #337,
#322, #244, #238.

**Lane W order — one at a time, nothing else on the box** (the #324 measurement is
*about* concurrency, so any overlapping job invalidates it):
W.1 core#324 (N≥20 alone, then N≥20 serialized) → W.2 core#342 Windows residual
(3 arms × 50 at `--retries 0`; **skip if 0.1 answered it**) → W.3 core#238 NUL probe.

---

## 5. Per item

Criterion-level detail lives in `.planning/ledger/<file>`. This table gives the
statement, the site, the shape, and — the part that matters — **the mutation that
must turn the acceptance test red.** An acceptance test that cannot fail is worse
than no test, because it certifies the defect.

### 5.1 `FerroxLabs/wayland-core` — 17 issues

| # | Statement | Defect site | Fix shape | Mutation that must turn it RED | Ledger |
|---|---|---|---|---|---|
| **339** | The @-ref secret guard is purely lexical, so a symlink named `notes.txt` inlines `~/.git-credentials`. | `at_ref_guard.rs:78-90` ("Purely lexical" at `:77`); `at_ref_resolve.rs:203`, `:304`, `:328`, `:339`; `at_ref_complete.rs:105`; **4th site `at_ref_send.rs:396`** | Resolve once, `File::open` once, guard the **opened handle's** identity (`same_file::Handle` — dev+inode / FileIndex+VolumeSerial) against the canonical path; canonicalize the ROOT too; never blanket-refuse symlinks. | `ln -s <outside>/.git-credentials notes.txt` → `resolve(AtRef::File)` must `Err(SecretBlocked)`; on `cfa89a9c` it returns credential bytes. Nine arms incl. a **symlinked-ROOT** arm (the only Linux-catchable proxy for the macOS `/var`→`/private/var` regression) and two wrong-refusal controls (an ordinary file and an ordinary **directory** must still inline). | `wayland-core-339.md` |
| **335** | @-refs escaping the root skip the gitignore check: `rel_to_root` returns `None` and the `if let Some(rel) &&` short-circuits. | `at_ref_resolve.rs:397-403`, `:194-198`, `:410-418` | **Maintainer decision (Q1).** Recommended: leave the capability, pin it with a test, inside the #339 change. | The pinning test must cover **both spellings** — absolute AND `..`-relative — because `@../../secrets/foo.txt` escapes identically. Land the pin **before** canonicalization so the canon change has something that reddens when `None` starts appearing for in-root paths. | `wayland-core-335.md` |
| **323** | **Both halves.** PLAN.md graded the @→tools direction as fixed by `d7b7c430`; `lane/atref` found the **tools→@** direction still live: eleven credential filenames (`.envrc`, `.pgpass`, `secrets.json/yaml/yml`, `credentials`, `credentials.json`, `release.keystore`, `signing.jks`, `deploy_rsa`, `deploy_ed25519`) refused to a user typing `@name` but **readable by the model** through Read/Grep/Bash. | `at_ref_guard.rs` 29-path table `:324-393` | `at_ref_guard`'s list is **deleted**; its rules move into `workspace_policy::is_secret_path_static`. One list, one owner. Shipped on `lane/atref`. | Ask Read/Grep/Bash for each of the eleven names and require a refusal — RED on every shipped release through v0.13.10. Non-regression: `the_attach_guard_denies_every_path_either_denylist_carries` must still pass. | `wayland-core-323.md` |
| **322** | Nested/vendored VCS object stores not secret-denied. | — | **REFUTED at `cfa89a9c`.** `is_vcs_store_dir` (`workspace_policy.rs:2699`) + `vcs_store_entry` (`:2560`) wired into **both** walk arms (`:2445` serial, `:2480` parallel). Remove from `lane/atref` scope. | Non-regression: delete the `vcs_store_entry` call from **either** arm and `nested_store_reaches_the_os_deny_list` must go red. | `wayland-core-322.md` |
| **244** | VFS Read of raw `.git/objects` permitted. | — | **REFUTED on the VFS.** `is_vcs_content_store` (`workspace_policy.rs:901`) from `SecretDenyFs::guard` (`vfs.rs:2046`); all nine trait methods route through it (`:2054-2096`). | — | `wayland-core-244.md` |
| ⚠ | **The 322/244 class is closed on the VFS, NOT on the @-ref surface.** `wcore-cli` reads with bare `std::fs` (`at_ref_resolve.rs:203`, `:328`; `at_ref_send.rs:396`) and never constructs a `WorkspacePolicy`. `is_secret_path_static` carries `/.git/config`, `/.git-credentials`, `/.git/hooks/` but **nothing for `.git/objects`**, and the `.git` skip at `at_ref_resolve.rs:315` is a directory-recursion skip only. `RepoMap::build` lacks the `.filter_entry(file_name != ".git")` that `scope.rs:180` has, so **`@symbol` walks `.git`**. Fold into #339; **stop asserting the class is closed.** `lane/atref` does **not** touch `at_ref_send.rs`. | | | | |
| **340** | MCP malware gate bypasses: `pipx run evil-pkg` resolves the package to the literal `"run"`; the code makes a provably false claim about its own WARN visibility; the runner list misses `.exe`/`.cmd` **and** `bunx`/`pnpm dlx`/`yarn dlx`/`npm exec`/`deno run npm:` entirely. | S1: `osv_check.rs:269-275`, `:188-192` (`infer_ecosystem` matches exactly `{npx, npx.cmd, uvx, uvx.cmd, pipx}`). S2a: `malware_gate.rs:148-151`, `osv_check.rs:487-490`, `transport/stdio.rs:509-512` | S1: verb-skip for subcommand-first `pipx` argv; strip trailing `.exe`/`.cmd`/`.bat`/`.ps1`; add the missing runners. **Do NOT generalise "skip first positional for PyPI"** — `uvx` is positional-first. S2a: delete the false sentences. | `parse_package_from_args(&s(&["run","evil-pkg"]), PyPI) == identified("evil-pkg", None)` — RED on `cfa89a9c` (returns `identified("run", None)`). Plus a `uvx evil-pkg` control that must STAY positional-first. **Replace the `infer_ecosystem("python") == None` control at `osv_check.rs:531`** — a closed-set match cannot catch the over-broad strip it exists for; use `npx.exe.sh`, `mypipx`, `pipx-wrapper`. | `wayland-core-340.md` |
| **338** | Untrusted plugin install can make git prompt on `/dev/tty`. `run_git` sets `stdin(null)`/`stdout(piped)`/`stderr(piped)` but has **no `.env()` call**; the clone URL is catalog-controlled and it fires from inside the TUI alt screen. | `plugin/quarantine.rs:289-296`, reached from `:114-167` via `marketplace.rs:315`, `:505`. Only the clone at `:114-129` carries `hooksPath`/`protocol.ext` hardening; `:134`,`:139`,`:152`,`:157`,`:170` carry neither. | Layers 1+2 as ONE change (§4.3), teardown decided in the same change. | ARM 1: assert over the `Command` `run_git` actually builds, via `Command::get_envs()`, driven through a real `quarantine_clone` — HEAD's env map is **empty** → RED. ARM 2: **reproduce the prompt on unfixed code FIRST**, then author the pty arm **with `credential.helper` configured** and observe it RED *after layer 1 is already in the tree*; assert the **specific auth-refusal class**, not "fails closed" — `run_git` already pipes stdout/stderr, so "nothing on the pty" is the default state of the world. ARM 3: a no-auth local clone still succeeds. | `wayland-core-338.md` |
| **325** | The nightly Windows soak **auto-closes its tracker issue from a failed run** — step-level `if: success()` is job-scoped. Recorded on run `33053333326` (2026-08-27, conclusion FAILURE) which still closed #319 at 08:56:03Z. | `.github/workflows/nightly-windows-soak.yml:215-216` (close), `:265-266` (report); jobs `:109`, `:341`, `:478` | A terminal job with `needs: [windows-soak, windows-live-acceptance, keyring-blob-size]`, `if: always()`. Close iff `needs.windows-soak.result == 'success'` and nothing is `failure`/`cancelled`; report only on `failure`/`cancelled`, **never on `skipped`**; declare `permissions: issues: write` explicitly (inheriting the default 403s **silently**). Adding `needs:` to the existing steps does NOT work. **hetzner's token has no `workflow` scope — this must be pushed from a host that does.** | **Seed an issue** with labels `['windows-soak','test-debt']` and the exact `[nightly-windows-soak] FAIL` title prefix and assert on *that issue number* — the close step is a no-op with zero matching issues, so "must leave any open issue OPEN" is satisfied by zero and the broken workflow passes. Arm 1: soak green + live-acceptance red → seeded issue stays OPEN. Arm 2: all-green → seeded issue CLOSED (proves the gate is not permanently red). **Arm 3 (candidate mode): soak + keyring skipped, live-acceptance green → NO FAIL issue filed.** The obvious gate reads `skipped` as not-success and would file a FAIL on every candidate dispatch — a new false signal from the fix for a false signal. | `wayland-core-325.md` |
| **350** | `the_streaming_bash_timeout_bounds_the_secret_deny_walk` flakes: the derived timeout (`walk/10` = 6-7 ms) is 2.6x below the host's measured one-timer allowance (15.8 ms; Windows tick ≈ 15.6 ms). | `bash/tests.rs:2768`, assertion `:2798-2803`, decl `:2750-2751` (plain `#[tokio::test]`, no gate) | Apply the same explicit `cfg(windows)` exclusion the attribution half already carries at `:2488-2521`. Do **not** widen the `* 3` factor. Do **not** use an `eprintln!` skip notice — nextest suppresses passing tests' stderr and `check-no-vacuous-cargo-test.py` only catches zero-*test* runs. | On Linux, move the manifest build back **outside** the timeout scope in `bash.rs` → the latency half must go RED. Then confirm on Windows that it is **absent from the nextest list** (excluded at compile time), not passing via a self-skip. A patch deleting the assertion must FAIL the first arm. | `wayland-core-350.md` |
| **324** | `concurrent_allow_and_deny_identities_do_not_interfere` flakes — **5** recorded occurrences (not 4; add run `33053333326`), always the same line. **MEASURE, DO NOT FIX**: nobody has ever run it alone, so product-race vs fixture-race is unestablished. | `live_fs_acl.rs:441`; allow assertion `:471-474`; deny `:475-478`; env gate `:28-38` | Instrumented run on `ferrox-win-msvc` **through the runner service** (Session-0 SSH logon reports `is_available() == false`): N≥20 alone, then N≥20 with the two `execute()` calls serialized, nothing else on the box. | **Before the deny arm counts as evidence, assert the deny process actually RAN.** `:475-478` is `assert!(!stdout.contains(MARKER))`, which **empty stdout satisfies** — exactly the recorded Windows AppContainer state where binaries never launch. Add a launched-marker written before the read attempt, or an exit-code assertion. If it is a product race: the red arm is two concurrent identities on one path where the allow arm loses the marker, and the fix must keep the deny arm denied. | `wayland-core-324.md` |
| **337** | The dangerous-lease e2e assertion is **structurally unsatisfiable**: `:168` demands `read_pid + cancel < 4s` while the nested budgets legally permit 4s + 4s. The observed 6.385s is inside the permitted envelope. | `dangerous_lease_e2e_test.rs:168`, with `:145`, `:151-152`, `:156` | Delete `:168`, or re-baseline from a second `Instant` after `:154` bounding cancellation only. **Do NOT delete `.config/flaky-allowlist.txt:57`** — it names the wrong assertion, but the `:97` `read_pid` mechanism is a real, separate, live flake (`TRY 1 FAIL … 'must publish its PID before expiry: Elapsed(())'`, 2/3 on untouched `9d3f33c3`). Rewrite the entry to name `:97`. | Two-directional. A mutation making lease expiry NOT cancel the tree must **still** turn the test red (via `:156`, `:160-163`, `:174-175`). Conversely a **2500 ms sleep before writing the pid files must NOT turn it red** — and does today. | `wayland-core-337.md` |
| **336** | `harness_tui_flow narrow_terminal_resize_stays_coherent_without_panicking` — `PtyHarness::resize` resizes the PTY only; the `vt100::Parser` stays pinned at 40x120 and the post-resize predicate is **already true from boot**. | `harness_tui_flow.rs:276-285`; parser `:198`; predicates `:471-474` **and `:488-492`** (byte-identical, same 5s budget; `:488-492` is byte-identical to `boot_to_workspace :325-329`); resizes `:469`, `:487` | `parser.set_size(rows, cols)` under the same lock (`vt100` pinned 0.15.2, has it), then replace **BOTH** predicates. Required property: **each predicate must be asserted FALSE against a frame captured immediately before its own resize, in the same test.** | **(a) Make `PtyHarness::resize` a no-op — today the test PASSES. That is the proof of vacuity.** After the fix both arms must fail. (b) Inject a render panic on `cols < 100` — must fail before and after. Rejected replacements: "every row ≤ 80 cols" (true by construction after `set_size`) and rail-hidden-at-120 (already true; `:450-456` says the rail never paints). | `wayland-core-336.md` |
| **314** | Filed defect (`grant_path`/`revoke_path`/`grant_workspace_capability` unpublished) is **FIXED** (`generate.rs:171-181`, 29 command branches, fixtures on disk, contract 1.22). **Residue D-1 STILL-REAL:** `emit_path_grant` skips the documented `workspace_policy` receipt on both refusal exits while the doc promises it unconditionally. | `main.rs:4196`, `:4222` (and `:4276`, `:4302`) vs `docs/json-stream-protocol.md:977-980` | Emit the receipt on every exit, before the refusal `Info` frame. **The primary deliverable is the missing test** — those three functions have ZERO tests and the four dispatch sites at `main.rs:6629-6665` are ungraded. | Drive the JSON stream with `grant_path` against a launcher started **without** `--allow-host-path-grants`; a `workspace_policy` frame must arrive — RED today. **Vacuity guard: assert the frame TYPE, not that a frame arrived — an `Info` frame always arrives.** Liveness correction: deleting the `grant_path` `WireSpec` from `spec.rs:258-260` makes `generated_artifacts()` **panic** (`generate.rs:2002-2021`), killing the test in the generator; use a generator-tolerated mutation (remove the fixture or the schema branch). | `wayland-core-314.md` |
| **342** | INV-2 save-during-edit data loss **fixed on Linux/macOS** (0 lost across 2880 attempts; `atomic_io.rs:189`, `:226`). **Residuals STILL-REAL.** | R1: `atomic_io.rs:251-254` (`exchange` → `Ok(Swap::Unsupported)` off Linux/macOS), fallback `:126-139`; also `vfs.rs:~505`, `~479`. R2: `inv2_round5_adversarial_test.rs:661` — `assert!(lost * 4 < interleaved)` is **arithmetically inert** (6.5% × 618 ≈ 40; 40×4 = 160 < 618). R3: same file `:630`, `:1024` ungated `assert_eq!(lost, 0)` running on required `CI (Array)`. | Close #342 only **after filing two successors**: (1) Windows/no-exchange — bracket check+publish with an exclusive open so a racing save fails LOUDLY, or make the degradation visible; (2) tighten `:661` to `assert_eq!(lost, 0)` on exchange-capable platforms, keeping the `interleaved > 0` vacuity guard. Factual correction for the record: `ReplaceFileW`'s `lpBackupFileName` **is** a caller-chosen displaced-file name; the in-tree objection on that point is wrong. The not-atomic and fails-when-open objections survive. | **Linux, cheap, decisive:** mutate `atomic_io.rs:189` to return `Ok(Swap::Unsupported)` on Linux and re-run `an_in_place_save_can_still_lose_to_the_final_rename` at `--retries 0`. Under the CURRENT assertion it is expected to **still PASS** — that is the proof the gate is inert. Under `assert_eq!(lost, 0)` it must go RED. **False-green guards:** `touch` every restored file after mutation/restore, and confirm each run's printed "N interleavings caught" is non-zero before believing any 0-lost. | `wayland-core-342.md` · cross-tracker twin of wayland#1155 |
| **253** | **Umbrella = NEEDS-DECISION. Buried defect = STILL-REAL (high):** Telegram forum-topic sends set `reply_to_message_id = <topic id>` and never set `message_thread_id`. | **Rare path:** `channel_send_transport.rs:236` (`reply_to: target.thread_id.clone()`). **Common (default agent-reply) path:** `channel_inbound.rs:97-100`, consumed at `:633` and `:651`; the doc at `:91-95` is **false for Telegram**. Missing field: `wcore-channels/src/outgoing.rs:7-20`. | Split the buried defect into its own bug issue. Add `thread_id: Option<String>` to `OutgoingMessage` as the **destination**, leaving `reply_to` as the quoted message. Add the field → let the compiler enumerate the two struct literals → **then audit `::text()` callers** (silent-default constructor at `outgoing.rs:23-31`). | (1) Serialize the outbound request for `telegram:<chat>:<topic>` → `message_thread_id == <topic>` **AND** `reply_to_message_id == None`; fails on both halves today. **(2) Inbound arm — drive `channel_inbound` with `thread_id = Some("42"), reply_to_message_id = None`. A rare-path-only test goes GREEN with `:633`/`:651` still broken.** (3) All chunks: a message long enough to chunk, assert on **every** part. (4) Slack regression triple produces byte-identical `thread_ts`. No live Telegram credential needed — assert on the serialized body. | `wayland-core-253.md` |
| **238** | No Windows reserved-name guard: bare `NUL` makes Write/Edit **discard the bytes while reporting success**. | `path_validation.rs:63-158` (`validate_user_path`, no reserved-name arm); blind mitigation `:152-156` | **Maintainer decision (Q5).** Recommended: a narrow **bare-`NUL`-only** guard as a distinct `PathValidationError`. **Do NOT build the textbook list** — measurement on Win 11 26200 refutes it. | H.1 (Linux, zero box time): `validate_user_path("C:\proj\NUL")` returns the new variant, **PLUS** the load-bearing wrong-refusal controls — `aux.txt`, `NUL.txt`, `COM1`, `con.json`, `C:\proj\nul.rs` must ALL still return `Ok`. *A guard was already written, unit-tested green, and discarded because its tests encoded the same wrong assumption.* W.3 (Windows): probe bare-`NUL` device behaviour on the build under test, and prove `fs::metadata("...\NUL").is_file() == true` so the `NonRegularFile` mitigation at `:152-156` is shown structurally blind. **Restate the gate as 26200-only; record that 20348 is unprobed.** | `wayland-core-238.md` |
| **113** | "Browser tool non-functional by default." | — | **REFUTED — 3 of 4 evidence claims fall.** `launch_camoufox` has a full production path (`tool.rs:302` → `supervisor.rs:471` → `:559`); `SidecarAutoInstall` is on by default; there is no `chromium.rs` and no chromium feature; the opaque dead end is closed at `tool.rs:588`. Only two stale doc lines remain (`provider.rs:3`, `:159`). Deny-by-default browsing is intentional policy. | Nothing to grade — this is a **closure recommendation**, and closing is a maintainer action (Q-113 in §8). | `wayland-core-113.md` |

### 5.2 `FerroxLabs/wayland` — the 12 PARTIALs (outstanding half only)

| # | What shipped in v0.13.10 | **What did NOT — this is the work** | Site | Mutation that must turn it RED |
|---|---|---|---|---|
| **388** | Output caps and reasoner replay now decided from what is known, not the alias named (`0cab1cf8`) | 4 of the 7 Expected-Behavior bullets — all **router-side** | `wcore-config/src/limits.rs` | **BLOCKED on flux.** Core cannot redden a router behaviour from this repo. |
| **434** | The replay socket is populated | By construction it covers only turn N+1; the alias-resolves-server-side path is not closed core-side alone | — | **BLOCKED on flux** — the router must declare the resolved model on the turn it resolves it. |
| **559** | Both root causes fixed and measured: turn-1 transient poisoning the cache prefix (#1168) and the OpenAI adapter dropping text on tool-result turns. Hit ratio 0.0358 → 0.6526. | The ticket's **own close condition**: one real 26-turn Desktop team run showing non-zero `cache_read`. The measurement so far is a 7-round-trip synthetic rig on flux-router. The sub-call-count half of ask 2 has no trace at all. | — | The 26-turn run is the test. A synthetic rig at 7 round-trips **cannot** exhibit the multi-turn prefix decay the ticket describes — it is not a smaller version of the acceptance, it is a different measurement. |
| **908** | Reasoning-tag leak fixed (`508405d4`) | Two other reported sub-symptoms, incl. the `Sandbox child timed out` half | `wcore-types/src/reasoning_filter.rs` | Name the two sub-symptoms on the issue before writing a test — an unnamed remainder cannot be graded. **Lane `finish-a`.** |
| **934** | Items 1 and 3: five gates that could not fail now discriminate; three wrong caps corrected against the boundary (Slack 39,000 → measured 4,000, `9026e648`); `cap_measured` is machine-readable | **Item 2 — the committed boundary probe never landed.** Slack + Discord were measured live 2026-08-27 but the probes were not committed. | `docs/delivery-semantics.md`; `wcore-channels-registry/tests/delivery_semantics_declaration.rs` | Set an adapter's declared cap one byte above the measured boundary → the probe must go RED. Slack and Discord are **not** credential-blocked; this is small and unblocked. **Lane `finish-a`.** (c5 stays `blocked`/maintainer — §7.) |
| **998** | Core-side enforcement complete: operator per-tool selection honoured at boot, on live add, and across a `list_changed` refresh; `Some([])` means none; identical across transports | Three reasons open: (1) `defer_config_mcp` installs no catalog refresher → **#1174**; (2) Desktop does not send the field on the ACP path; (3) `wcore-acp` has no MCP surface at all | `bootstrap.rs:1810`; `engine.rs:5190-5192`; `tool_proxy.rs:447` | (1) is #1174 below. (2) is **BLOCKED on desktop**. (3) is core work with no ticket of its own — file one. |
| **1088** | Core built the typed event and corrected the generated corpus row; the guard now lives **inside** `generated_artifacts()` so a violating corpus cannot be emitted at all | The user-visible half | — | **BLOCKED on desktop.** |
| **1150** | Compaction half: unlisted models no longer get a fabricated 200,000-token window | "Truncate/summarize large fetched content": `WEB_FETCH_MAX_RESPONSE_BYTES = 256 * 1024` passes a 50,000-char result through untouched | `wcore-tools/src/web_fetch.rs:78` | Fetch a 50,000-char body and assert the tool result is bounded well below it. RED today. **Lane `finish-a`.** |
| **1151** | The Bash tool now discloses which shell it really runs, via the tool description | (1) The real fix — use a real bash when present, never `System32\bash.exe` — is **#1164**, deliberately held for live Windows verification; silent `echo A; echo B` exit-0 persists. (2) Out-of-order transcript assembly | `wcore-tools/src/bash.rs:549` → `registry.rs:501` | (1) needs the Windows box. (2) is **BLOCKED on desktop** — the symbols do not exist under `crates/` at all. |
| **1155** | Guarded writes publish via atomic compare-and-exchange; 160 losses → 0 at n=200/arm; suite 14/48 → 0/48 | R1: **Windows** has no exchange primitive and degrades to re-check-then-rename — the race stays open. R2: `an_in_place_save_can_still_lose_to_the_final_rename` tolerates up to **25%** data loss, dates to v0.13.0, and was **never re-graded** after the exchange landed. | `wcore-config/src/atomic_io.rs:251-254` (`Swap::Unsupported`), `:69` | Same as core#342 — this is the same defect on the other tracker. **Lane `finish-b`.** |
| **1156** | Product half: `acp serve` profile children bound to a parent-death channel, live-proven | The ticket asked for the **test supervisor** to own the tree. Five test sites still spawn `acp serve` unbound — including the one that spawns the supervisor itself with `Stdio::null()`. The fix kills the child; nothing kills the server. | `profile_router_live.rs:99-115` | Kill the supervisor mid-run and assert **zero** surviving `acp serve` processes. A test that only checks the child is the exact gap. **Lane `finish-b`.** |
| **1172** | Detection: `ServedWindowTracker` learns the served window from `usage.prompt_tokens`, names the shortfall, and uses `emit_info` **not `warn!`** so it reaches the user with `RUST_LOG` unset | Compensation: the learned window feeds only the pressure gauge. The pre-flight guard and autocompact still re-resolve without it. Feeding it today would **brick the run** — see #1179. | `context_window.rs:223`; `engine.rs:12790` (guard); `engine.rs:8199` (autocompact) | Tracked as **#1179**; fix the buffer arithmetic first. **Lane `finish-b`.** |

### 5.3 `FerroxLabs/wayland` — the 9 filed during the cycle

| # | Statement | Site | Fix shape | Mutation that must turn it RED |
|---|---|---|---|---|
| **1174** | Under `defer_config_mcp` — **the mode the Wayland Desktop host runs in** — `mcp_refresh_configs` stays empty, `McpCatalogRefresh::is_empty()` is true, no refresher is installed, and `tools/list_changed` is silently ignored for **every** server for the life of the session. | `engine.rs:5190-5192`; `bootstrap.rs:1915` (constructed), `:3528` (installed) | Install a refresher in the deferred path too, **carrying `McpServerConfig`** — see §4.3. | Boot with `defer_config_mcp`, have a server emit `notifications/tools/list_changed`, assert the catalog changed. RED today. **Assert the per-tool allowlist is still enforced after the refresh** — that is the guard against fixing this into a privilege escalation. |
| **1175** | Every runtime-added MCP server builds a brand-new `McpManager` that never enters the boot-captured `McpCatalogRefresh.managers`; only boot managers are polled. `McpCatalogRefresh`'s fields are plain `Vec`/`HashMap` behind an `Arc` with `&self` methods only, so **there is no way to register a manager after construction**. | `wcore-cli/src/main.rs:5605`, `:5678`, `:3822`, `:3880`, `:3456`; `engine_bridge.rs:2535`; `bootstrap.rs:1915`, `:1853`; `tool_proxy.rs:442-447`; `plugins/mcp_delivery.rs:57/69/81` | Interior mutability (or a registration channel) on `McpCatalogRefresh`, **threading `McpServerConfig` into `server_configs` in the same change.** | Add a server at runtime, emit `list_changed`, assert the catalog updated — RED today. **Paired negative arm: a runtime-added server carrying an operator allowlist must NOT gain its full tool set after the refresh.** Without that arm the fix silently activates the allow-all read at `tool_proxy.rs:447`. |
| **1176** | Both model-limits guards are blind to provider-native passthrough ids: the freshness script cannot evaluate `if`-chain families and the drift test walks routed aliases only. Passthrough ids in `if`-chain families are covered by **neither** — the exact #165 class. | `wcore-config/src/limits.rs`; `scripts/check-model-limits-freshness.py`; `release.yml` `prepare-release`; `compact.rs:340` | Make the guards enumerate what the code actually branches on, not what the alias table lists. (The three found arms landed on `lane/wlimits` @ `a3da73c5`; the issue is about the guards.) | Add a passthrough id in an `if`-chain family with a deliberately wrong ceiling → both guards must go RED. Today both stay green — that is the defect. |
| **1177** | `nick-fields/retry@v3` `max_attempts: 2` re-runs the suite and attempt 2 **overwrites** `target/nextest/ci/junit.xml`, erasing a genuine attempt-1 failure. | `.github/workflows/ci.yml:1604`; `.github/scripts/grade-retry-flakes.sh` | Write per-attempt junit paths and grade the union. | Fail attempt 1 deliberately, pass attempt 2, and assert the failure is still present in the graded artifact. **Needs a `workflow`-scope token — hetzner's does not have one.** |
| **1178** | A `base_url` with the canonical `/v1` suffix builds `/v1/v1/chat/completions` and 404s with nothing naming the cause. | `openai_defaults()` | Normalise the join, or detect and report. **Per the no-hardcoded-quirks rule this belongs in `ProviderCompat`, not in a URL-sniffing conditional.** | Configure `base_url` ending in `/v1` and assert the built path has exactly one `/v1`. Plus a control: a base URL **without** `/v1` must still build correctly. |
| **1179** | Absolute context buffers saturate to zero on a small served window: at 4,096 `input_ceiling()` saturates to 0 (guard fires every turn) and `autocompact_threshold` = 2,867 falls **below core's own 3,118-token baseline turn**. | `input_ceiling()`; `output_reserve` 20,000; `emergency_buffer` 3,000 | Make the buffers proportional with an absolute floor. **Gates #1172's compensation half — do not feed the learned window in until this is fixed.** | Set a served window of 4,096 and assert `input_ceiling() > 0` and `autocompact_threshold` above the measured baseline turn. Both fail today. **Lane `finish-b`.** |
| **1180** | `approval_bridge.resolve(...)` in the active-turn handler is the last ungraded approval seam — no test would notice if it were deleted. | `wcore-cli/src/main.rs:6255`; existing graded arms at `approval_resume_contract.rs:415`, `json_stream_approval_test.rs:190,279` | Add the missing test. | **Delete the `approval_bridge.resolve` call — the new test must go RED.** That is the whole acceptance; nothing else is being claimed. |
| **1181** | Four orphaned lane branches carry unmerged fixes and are not ancestors of main: `lane/walk-parallel` (`13a81ab8`), `lane/winpath` (`4089798c`), `lane/tools-bash` (`c7aeaf2d`), `lane/win-fix` (`c5ce3857`), 6–27 days stale. Two are in the "assertion that cannot fail" / "green check that ran nothing" classes. | branch tips | A **per-branch** written outcome: rebase-merge, superseded-by-<sha>, or obsolete. Not a bulk disposition. | `git merge-base --is-ancestor <tip> origin/main` must succeed, or the issue must carry a written superseded/obsolete verdict naming the commit that replaced it. **Add `lane/finish-a` and `lane/finish-b` to this issue — they are unpushed and would become orphans on a box loss.** |
| **1182** | `contained_construction_does_not_walk_the_workspace` establishes its own liveness with a wall-clock ratio; under load `walk=20.4ms` vs `walk_empty=8.2ms` compress and the **control declares itself dead**. | `wcore-tools/src/workspace_policy/tests.rs:66`; `.config/flaky-allowlist.txt` | Replace the wall-clock liveness with a structural one (count the walk, do not time it). Same family as core#337 and core#336. | Make the contained construction walk the workspace → RED. **And under load → still RED, not "dead".** A liveness check that can report "dead" instead of "failed" is the defect. |

### 5.4 `FerroxLabs/wayland` — 5 older, still open

174, 305, 863, 1164, 1165. **305** c4 and all three of **863**'s open criteria are
blocked on other lanes (§7). **1164** is the real `System32\bash.exe` fix, deliberately
held for live Windows verification (§9). See their ledger files.

---

## 6. Release sequencing

**Recommendation — the maintainer may overrule this.**

**0.13.11 = the `lane/atref` credential fix ALONE, cut as soon as it is gated.**

`lane/atref` @ `6d130a62` closes a **live credential exposure present in v0.13.10 and
every earlier release**: eleven credential filenames (`.envrc`, `.pgpass`,
`secrets.json`, `secrets.yaml`, `secrets.yml`, `credentials`, `credentials.json`,
`release.keystore`, `signing.jks`, `deploy_rsa`, `deploy_ed25519`) were refused to a
user typing `@name` but **readable by the model** through Read/Grep/Bash. It also
closes core#339 (a symlink named `notes.txt` hands `~/.git-credentials` to the model)
and core#335. It is done, gated, and pushed.

Holding it behind eleven other fixes trades user safety for tidiness. Shipping it
alone also makes the release trivially reviewable: one commit, 7 files, +493/-108,
one crate boundary moved.

**Everything else = 0.13.12.**

**If the maintainer overrules and batches it into one release**, then: the Lane W
serialization (§4.4) and the Lane Z corpus regen (§4.4) become the critical path, the
Windows merge freeze (Q7) has to be declared before any of it starts, and the
exposure stays shipped for the length of that path. Say so on the issue, so the
decision is recorded rather than re-derived.

**Release-time gates that must be green either way:** `just ledger-check-live` runs in
`prepare-release` (`.github/workflows/release.yml:96-105`) and every publishing job
depends on `prepare-release`, so a FAIL stops the release rather than being advisory.
It needs `LEDGER_ISSUES_TOKEN` (§8).

---

## 7. Blocked on someone else

15 `blocked` criteria. Zero are owned by `core` — the gate forbids it.

### Needs Desktop code (4)

| Issue | Criterion | Contract |
|---|---|---|
| wayland#1088 | c2 — the chat interface stops reporting Read/Glob/Write/Edit as restricted | Core emits the typed event; Desktop renders it. Ticket carries `needs:desktop`. |
| wayland#1151 | c3 — the transcript stops assembling out of order | Entirely Desktop's; **the symbols do not exist under `crates/` at all.** This half was never core's. |
| wayland#305 | c4 — Desktop autodetects a local WSL Core and offers detected-vs-manual endpoint/key settings | The probe needs **no** Core change (`/openapi.json` is already unauthenticated); the settings UI and the allowlist popup are Desktop's. |
| wayland#998 | c5 — Desktop sends the per-tool field on the ACP path | Desktop's wire types drop the field before it reaches core. Core cannot honour a selection it never receives. |

### Needs Flux (5)

| Issue | Criterion | Contract |
|---|---|---|
| wayland#388 | c4 — the remaining four Expected-Behavior bullets | Router-side retry/stall behaviour inside the Free Models Router. `needs:flux`. |
| wayland#434 | c3 — the alias-resolves-server-side path closed end to end | The router must declare the resolved model **on the turn it resolves it**. |
| wayland#863 | c3 — F1: Elevation unreachable by default from `flux-fast`/`flux-standard`/`flux-reasoning`/`flux-auto` | Flux replied that F1 holds with per-request opt-in, but **core has no way to verify a server-side deployment** and later recorded F1 as unconfirmed. Needs a verifiable statement, not a reply. |
| wayland#863 | c4 — F3 server half: `loop_owner`/client-nonce requests bypass or vary the semantic cache | Flux reports it shipped; core cannot observe a cache bypass from the client. |
| wayland#863 | c5 — F4: the bandit routes `loop_owner` to a tool-calling-capable arm, or a `flux-agentic` alias exists | Flux deferred F4 explicitly and disclosed it. Elevation hard-skips tool turns so there is no collision exposure, but the routing floor is absent. |

### Needs a credential we do not hold (1)

| Issue | Criterion | Contract |
|---|---|---|
| wayland#934 | c5 — every adapter's declared cap verified against the real platform limit | Seven adapters still `cap_measured = no`. **There is no Twilio or Meta credential at all**, and the Matrix token was found dead on 2026-07-31. Slack and Discord are *not* blocked — that half is item 2 in §5.2 and is small and unblocked. |

### Needs the Windows box

Not modelled as `blocked` criteria (they are core-owned and doable, just serialized
on one machine) — see §9.

### Needs a maintainer decision (5 blocked + 1 not-met) — §8

core#113 c5, core#238 c5, core#253 c1, core#314 c5, core#335 c1, and
core#338 c4 (`not-met`, owner `maintainer`).

---

## 8. Maintainer decisions outstanding

Each carries a recommendation. Nothing here should be picked silently by a lane.

> **TAKEN 2026-08-29 — see [`.planning/DECISIONS.md`](DECISIONS.md).** Every row in the
> table below has since been decided and recorded there with its reason. This section is
> kept for the reasoning that led to each recommendation; `DECISIONS.md` is what a lane
> should read for the choice itself. The criteria ledger cites `DECISIONS.md`, not this
> table, for `core#335 c1`, `core#238 c5`, `core#253 c1` and `core#338 c4`.

| ID | Decision | Recommendation | Reason |
|---|---|---|---|
| **SECRET** | Create `LEDGER_ISSUES_TOKEN` — a PAT with issue-read on **both** `FerroxLabs/wayland` and `FerroxLabs/wayland-core`. | **Create it.** | `release.yml:96-105` runs the live coverage arm with `secrets.LEDGER_ISSUES_TOKEN \|\| secrets.GITHUB_TOKEN`. The repo-scoped `GITHUB_TOKEN` **cannot see the second tracker** — which is exactly the miss in §3. Without the PAT the release-time check either fails (blocking the release) or, in the worse case, reaches wayland and returns an empty-but-successful list for wayland-core: the `reached == 0` guard is summed **across both trackers**, so that case would have to be caught by the per-issue ORPHAN path instead. **Recommend also splitting `reached` per tracker and failing on a per-tracker zero** — that closes the one hole in the fail-closed claim. |
| **Q1** (core#335) | @-refs that escape the workspace root: (A) leave, pin with a test · (B) upward `.gitignore` discovery · (C) refuse escaping paths | **A.** | The security half is already closed by the #323 union — `@~/.ssh/id_ecdsa`, `@~/.aws/credentials`, `@/root/.git-credentials` are refused on the absolute path directly. B does not exist today. C removes a real capability users experience as a regression. **Whatever is chosen must be phrased over *escaping* paths, not absolute ones** — `@../../secrets/foo.txt` escapes identically, so "refuse absolute paths" would not close it. |
| **Q5** (core#238) | Narrow bare-`NUL` guard, or won't-fix? (textbook reserved-name list is **REFUTED** by measurement) | **Build the narrow guard.** | Bare `NUL` makes Write/Edit discard the bytes **while reporting success** — silent data loss with a false success claim. The narrow guard converts that into a refusal and costs nothing measurable. The textbook list would refuse `aux.txt`, `COM1`, `NUL.txt`, `con.json` — real addressable user files; on Win 11 26200 only bare `NUL` is still a device. **That guard has already been written, unit-tested green, and discarded once.** Either way, record the 26200 measurement on the issue so it is not refiled a third time. |
| **Q2** (core#340 D) | Should the malware gate refuse *indirect* runners (`sh -c "npx …"`)? | **No — and say so in the doc.** | "Detect a shell and refuse" is a game of spellings (`/bin/sh`, `env sh`, `busybox sh`, a wrapper script) that will not hold, and a half-fix buys false coverage. **Reachability is real and worse than filed:** not only a hand-edited `config.toml` but `ProtocolCommand::AddMcpServer` (`commands.rs:388`) lets the desktop host inject an arbitrary command+args at runtime, validated only for **LENGTH** (`main.rs:3546-3583`). |
| **Q3** (core#340 B) | How does the fail-open notice actually reach a user — `warn!`→`error!`, or a typed protocol frame? | **A protocol frame, not a log level.** | `main.rs:1372-1380`: the TUI branch is **file-only** ("NOTHING may reach stdio — not even an error"); `:1104-1108`: the json-stream consumer does not read stderr; only `:1381-1390` (headless/REPL) tees to stderr at ERROR. **The TUI is the primary MCP-launch surface**, so `error!` fixes 1 of 3 shipped modes while an `assert level == ERROR` test goes green for all three — certifying a *new* false claim. Also: the pattern being copied (`wayland-ijfw/src/mcp.rs:434-467`) wraps a **sync** fn and asserts `levels.len() == 1`; the target `refuse_if_malware` (`malware_gate.rs:155`) is **async** under a real HTTP backend, and `with_default` is **thread-local** so under `flavor = "multi_thread"` it captures nothing. Ship slice 2a (doc honesty) now; decide visibility with Q4. |
| **Q4** (core#314 D-2) | Should protocol refusals be typed events? | **Yes — with Desktop, as a contract minor bump.** | Today refusals are untyped `ProtocolEvent::Info` prose with an empty `msg_id` (`events.rs:982-985`), no `grant_id` echo, no machine-readable reason — despite `goal_control_refused`, `quiesce_refused`, `set_mode_refused` existing as precedents. It widens the event union, forces a contract minor bump and moves `schema_digest` on a cross-lane boundary. Open with Desktop on FerroxLabs/wayland#1099. |
| **Q6** (core#253) | Schedule or park the umbrella? | **Keep open, unscheduled. Split the buried Telegram defect out today.** | The umbrella requests **absent** behaviour across 8 sub-designs and a 12-line acceptance matrix; nothing in it describes broken behaviour. Its slice 2 carries a **breaking migration** (`SHAPE_FIELDS` 13→14, `ADMISSION_SHAPE_VERSION` admission-v2→v3) that invalidates every `acknowledge_open_admission` token an operator has written — every open-admission channel refuses to start until re-acknowledged. The Telegram defect needs none of it. |
| **Q7** | Declare a Windows merge freeze for Lane W? | **Yes — a declared window, opened only after Lane 0.1 has been read.** | Sean-only: it stalls every other lane's `CI (Array)` (~42 min per PR on the single box). |
| **Q-113** | core#113: close as refuted, recording deny-by-default as the decision. | **Close.** | 3 of 4 evidence claims fall against `cfa89a9c`. Only Sean closes issues in this repo. |
| **Q-338c4** | core#338: deny `/dev/tty` to the clone · clear `credential.helper` for quarantine clones · label quarantine-originated prompts | **Deny `/dev/tty`** (`setsid`), decided **in the same change as layer 1** — §4.3. | Layer 1 alone makes the acceptance test green while the `credential.helper` path stays open. This is `not-met`/owner `maintainer` and gates the whole fix. |

---

## 9. Platform reality

**There is no local Windows gate, and every late defect in the last release hid on
Windows.**

| Platform | How we reach it | Cost |
|---|---|---|
| Linux | hetzner-dsm, `/root/wayland`. The whole suite in **2–3 min**. | free |
| Linux (CI) | `CI (linux-containerized)` | **67 min** — the actual CI critical path |
| macOS | **CI only.** A `lane/**` branch carrying `[ci-darwin]`. No local macOS gate. | **62 min** |
| Windows | **One** self-hosted runner: `ferrox-win-msvc`, a runner *service* on SeanDesktop, Win 11 build **26200**. Hosted image is 26100. **Nothing in the fleet is 20348.** | `CI (Array)` ≈ **42 min** per PR, and it is a **required check** |

Windows is a **serialized resource**. `nightly-windows-soak.yml:66-67` serializes
nightly runs only against **each other**, not against `ci.yml`. The workflow's own
comment at `:459-462` says it: *"Nothing serializes them; if a measurement produces an
inexplicable result, check contention first."*

Session-0 SSH logon reports `is_available() == false`, so **#324 must run through the
runner service**, not over ssh.

**Verifiable only on the Windows box:**

| Item | What must run there | Notes |
|---|---|---|
| core#324 | N≥20 alone, then N≥20 serialized, nothing else on the box | The measurement is *about* concurrency; any overlapping job invalidates it |
| core#342 / wayland#1155 R1 | 3 edit arms × 50 runs at `--retries 0` | **Skip if Lane 0.1 answers it from retained CI logs** |
| core#238 | Bare-`NUL` device probe + `fs::metadata(...).is_file()` on the build under test | Minutes. **26200-only; 20348 unprobed — do not generalise.** |
| core#350 | Confirm the latency half is **absent from the nextest list**, not self-skipping | The Linux red arm proves the fix; Windows proves the exclusion |
| wayland#1164 / #1151 | The real `System32\bash.exe` fix | Deliberately held for live Windows verification |
| core#325 | Dispatch replay (`timeout-minutes: 60`) | Also needs a `workflow`-scope token |

**hetzner's GitHub token has no `workflow` scope.** core#325 and wayland#1177 both edit
`.github/workflows/**` and cannot be pushed from hetzner. Push those from the Mac.

---

## 10. Hard-won rules

Each of these has cost real time.

- **Never run cargo on the Mac.** hetzner-dsm for Linux; SeanDesktop for Windows;
  macOS via CI. `cargo fmt` is the only safe local command.
- **`--profile ci` does NOT imply `--all-features`.** A green `ci` profile run says
  nothing about feature-gated code.
- **`cmd | tail` reports `tail`'s exit status.** A failing command piped into `tail`
  looks like a pass. Use `set -o pipefail`, or capture the status before piping.
- **Check ancestry with `git merge-base --is-ancestor <tip> origin/main` for every
  lane tip, AND diff local vs origin for unpushed commits.** `lane/finish-a` and
  `lane/finish-b` exist only on hetzner right now. A lane that looks landed may be one
  box failure from gone.
- **gitleaks reads its config from the working directory, not from the pushed
  branch.** A local pass is not a CI pass.
- **Never `git stash`.** There is one `refs/stash` across ~290 worktrees. Two parallel
  lanes have popped each other's work. Use a patch file or a scratch commit.
- **`PathBuf::join` uses the PLATFORM separator.** A path assembled on Linux and
  compared against a Windows spelling will not match, and the test will look like a
  product bug.
- **Never push while that branch's checks are running** — the push cancels the
  in-flight run. `gh run list` before every push; batch and push once.
- **`touch` every file you restore after a mutation.** Restoring with `mv`/`cp` gives
  an older mtime, cargo skips the rebuild, and the "restored" run measures the
  **mutated** binary. Fake red, or worse, fake green.
- **A green check that ran nothing is not a pass.** Read the *steps* for `skipped`; a
  run conclusion never claims anything actually executed. A retried failure reports
  the run as SUCCESS, so conclusion-filtered surveys are biased clean — grep logs for
  `TRY n FAIL`.
- **`warn!` never reaches the user.** With `RUST_LOG` unset only ERROR hits stderr,
  and in the TUI **nothing** reaches stdio. A log-level bump can never fix "the user
  is not told" (Q3).
- **Empty output reads as absent.** A query that fails silently returns nothing, and
  nothing reads as "no problem". Run a known-positive control in the same call.
- **A control must contain the surface.** Pick known-bad by whether it *has* the
  feature, not by date.
- **Observe the red arm before trusting the control.** Every mutation in §5 is
  *specified*, not *run*.

---

## 11. What this plan does NOT cover

- **Any issue outside the 43 in §2.** No sweep has been run for mislabelled or
  unlabelled issues on either repo. **The filter bug described in §3 may still be
  hiding others** — the ledger's coverage arm closes it for *open* issues in both
  trackers, but auditing the queue tooling itself is out of scope here.
- **The core#253 umbrella feature.** Only the buried Telegram defect is planned.
- **The Windows sandbox strategy.** Closed and not to be re-opened: no sandbox on
  Windows, never chase AppContainer again. core#324 is a test-and-ACL-application
  question *inside* the existing backend, not a strategy question.
- **Server 2019/2022 build 20348.** Nothing in the fleet runs it. The core#238 and
  core#324 findings are **26200-only and must not be generalised.**
- **A rate for core#342 on Windows.** Nobody has that number. Lane W.2 schedules the
  measurement, and this plan **forbids relaxing the only assertions that could catch
  it** before the number exists.
- **Desktop-side and Flux-side work** (§7). Core states the contract; it does not plan
  the other lane's work.
- **Verification of any acceptance test in this document.** Every mutation in §5 is
  *specified*, not *observed*. Run the red arm before believing the control.
- **Any claim that an item is done.** This file is the *plan*. `.planning/ledger/` is
  the *state*. If they disagree, the ledger is right and this file is stale.

---

## 12. Ready to close — the 2026-08-29 re-grade

Graded against `origin/integ/next` @ `43848f75` (the 0.13.12 integration tree, all
sixteen lanes merged including `lane/session-tickets`). `scripts/check-criteria-ledger.py`
passes `--self-test` and `--offline`; the live run reports **zero** coverage gaps, zero
unresolvable `met` evidence and zero orphans. Every one of its 17 findings is the same
shape:

> DIVERGENCE: … marks every criterion met, but … is still open.

That is the gate working. **Only the maintainer closes an issue in this repo**, so an
issue whose work is finished cannot go green until they do. These seventeen are the
close-list, not a defect list:

| Tracker | Issues |
|---|---|
| `FerroxLabs/wayland` | #908, #1156, #1172, #1174, #1175, #1176, #1177, #1178, #1179, #1180, #1182 |
| `FerroxLabs/wayland-core` | #323, #325, #335, #338, #340, #356 |

Two of them carry a `superseded` residual rather than a plain sweep, and the successor
must be open when they close — both are: `core#340 c3 → core#354` (the OSV strict /
permissive knob) and `wayland#934 c6 → core#360` (the WhatsApp bridge cap). Neither
issue may be closed by anyone acting on this table alone; it records readiness, not
permission.

Until they close, the live gate stays red on these seventeen and green on everything
else. Do NOT "fix" that by marking a criterion not-met.
