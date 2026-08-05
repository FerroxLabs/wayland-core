# lane/boot-walk — SUMMARY

**Branch:** `lane/boot-walk`  **Base:** `c9ab048b952c5bc74c75ea8f76df06788408de59`
**Verdict:** goal ACHIEVED for the boot walk. Two of the brief's premises were FALSE and are
refuted with evidence. Two follow-on defects found and left open (named below, not fixed).

Evidence detail lives in `evidence/boot-walk-NOTES.md`.

---

## 1. What the brief said, and what is actually true

The brief said first paint costs **two full recursive walks**, and named them
`wcore_repomap::scope::scope_files` (walk 1) and
`wcore_tools::workspace_policy::project_committed_secrets` (walk 2).

**"Two full recursive walks" is TRUE** — and I proved it at path level, which the brief did not.
In a probe tree with a `.gitignore`d `node_modules/` of exactly 8,000 dirs, the directory
`node_modules` itself is opened **exactly 2** times and `node_modules/dN` **16,000** times. A
gitignore-respecting walker cannot produce a single one of those opens.

**"Walk 1 = repomap" is FALSE.** `scope_files` has two callers, both in `IndexStore`
(`store.rs:518` refresh, `:664` verify), and the only production caller of either is the explicit
`index` subcommand (`index_cmd.rs:105`). `RepoMap::build`'s production callers are `/repomap`,
`@`-refs and the RepoMap tool. None runs at boot. Control: `grep -rn WalkBuilder crates/` returns
exactly three construction sites, so the search finds the known positives and there is no fourth
walker to attribute walk 1 to.

**The real second walk is the recursive file watcher.** Attributed by PID inside a single
process: one thread performs the 150 dir opens of `project_committed_secrets` and **zero**
`inotify_add_watch`; a second thread performs 300 dir opens and **all 164** `inotify_add_watch`
(ground truth: 162 dirs). That is `notify` `RecursiveMode::Recursive` at `watch.rs:127`, armed
from `engine.rs:4888 install_file_watcher_eventually`. The scoping agent was right that the
inotify traffic came from there, and right to doubt the repomap attribution.

**Only one of the two blocks first paint.** `Backtrace::force_capture()` inside
`project_committed_secrets` gives
`project_committed_secrets ← compute_secret_deny ← WorkspacePolicy::contained ←
bootstrap::build_scoped ← wcore_cli::tui::splash_while ← wayland_core::run`.
The watcher is on a detached `std::thread` and does not block paint (it does contend for IO).

## 2. The finding that changed the decision

The brief framed this as a trade: prune the walk and reopen a sandbox hole, or keep the cost.
**There is no trade, because the walk's output was dead.**

`project_committed_secrets` filled a cached `secret_deny` field exposed by
`pub fn secret_deny_paths()`. That accessor has **zero production call sites**. The only
production consumer of a secret-deny list is `bash.rs:133`,
`manifest.fs_read_deny = p.secret_deny_paths_dynamic()`, and `secret_deny_paths_dynamic()` never
reads the cached field — it recomputes everything per Bash exec. #234 moved enforcement there to
close a TOCTOU gap and left the frozen list behind. The in-process file tools use a separate
per-access lexical predicate (`is_project_secret`), not the list.

Absence proved with a known-positive in the same invocation: the control
(`secret_deny_paths_dynamic()`) returns `bash.rs:133` plus 14 test hits; the target
(`secret_deny_paths()`) returns only tests and doc comments.

So the TUI was blocking first paint on a full no-prune traversal of the workspace to compute a
value nothing read.

**The deliberate no-prune at `workspace_policy.rs:828-836` remains fully intact.** It was never
the problem, and nothing about it changed.

## 3. Decision and cross-audit

Cross-audited 4 ways with the corrected premise. Each leg probed with the real question, and each
returned a substantive reply (8.0K / 19.3K / 1.6K — no empty votes; kimi's bullet-prefixed vote
and codex's repeated final block were both handled by extracting unanchored and taking the last
match).

**Unanimous CHOICE: A — delete the dead frozen list.** Shared reasoning: a stale `pub` deny-list
accessor sitting beside the correct dynamic one is not inert, it is a trap for the next caller who
greps for "secret deny" and picks the weaker of the two. Options C and D optimize or decorate a
computation whose output is discarded, and both keep the trap.

My adversarial pass added one thing the panel missed: `with_project_secret_deny()` must **keep**
setting `secret_read_deny_required`, because that flag is live (it gates the dynamic walk and
`bash.rs:498/578`). Only the list-extension is dead. The fix reflects that.

## 4. What landed

| Commit | Change |
|---|---|
| `227b26c4` | remove `secret_deny` field, `secret_deny_paths()`, and the eager `compute_secret_deny` calls in `trusted_local` / `contained` / `delegated_mutation`; `with_project_secret_deny` keeps only the flag |
| `5ece008e` | re-point every frozen-list test onto `secret_deny_paths_dynamic()` |
| `938eac7f` | regression pin + remove `readable_canon_roots`, which the fix left callerless |

Files changed vs merge-base `c9ab048b` (4): `crates/wcore-tools/src/workspace_policy.rs`,
`crates/wcore-tools/src/workspace_policy/tests.rs`,
`crates/wcore-agent/tests/workspace_bash_deny.rs`, and the NOTES file.
**No shared-fence file was touched** (`wcore-cli/src/lib.rs`, `main.rs` untouched).

Three tests needed more than a rename, and each is called out in its own doc comment rather than
quietly changed:
- `trusted_denies_wayland_profile_credentials` materialized the list *after* restoring
  `WAYLAND_HOME`. Safe against a construction-time list; wrong against one that reads the env at
  call time. The call moved above the restore.
- `dynamic_deny_catches_post_bootstrap_secret_remote` loses its frozen-list half, whose only
  purpose was to demonstrate the frozen list was stale.
- `dynamic_deny_local_keyboard_unchanged` compared dynamic against frozen; it now states its real
  claim directly (a local keyboard posture puts nothing under the workspace root in the list).

## 5. Measurements

Before/after are the SAME build config on the same host, A/B'd by toggling the eager walk back on
and rebuilding — so the differential isolates exactly this change.

| Metric (probe tree: 10,013 dirs / 30,020 files, 8,000 dirs gitignored) | BEFORE | AFTER |
|---|---|---|
| `openat` on `node_modules/dN` | 16,000 | **8,000** |
| `openat` anywhere in the workspace | 20,031 | **10,017** |
| wall span, process start → last workspace `openat` (under strace) | 5.887 s / 5.725 s | **3.686 s / 4.719 s** |
| in-process duration of the blocking walk (instrumented) | 294–367 ms | **not executed** |

Exactly one full traversal removed. The residual 10,017 is the detached watcher.

**I could not measure true time-to-first-paint and am not going to report a number for it.** The
TUI does not paint headless on this host — with stdin on `/dev/null` under `script` it never
enters raw mode, and the captured log contains only the vault warning. The brief's 10.63 s figure
is therefore not something I reproduced; what I did reproduce is the walk, its boot-path
attribution, and its removal.

## 6. Gates

| Gate | Result |
|---|---|
| `cargo fmt --all` (Mac) | clean |
| `cargo metadata --locked` | `METARC=0` |
| `cargo check --workspace --all-targets` | `CHECKRC=0`, 0 errors |
| `cargo clippy -p wcore-tools --all-targets -- -D warnings` | `RC=0` |
| `cargo test -p wcore-tools` | **1238 passed, 0 failed, 5 ignored, 0 filtered out** |
| `cargo test -p wcore-agent --test workspace_bash_deny` | 3 passed, 0 failed, 0 ignored, 0 filtered out |

**Unrun cells / known reds, reported not hidden:**
- `cargo clippy -p wcore-agent --all-targets -- -D warnings` is **RED, 7 errors**, all pre-existing
  and none in a file I touched: 4 × `needless_borrows_for_generic_args` in
  `tests/user_model_identity_wire.rs` and 1 × `needless_update` in
  `tests/cache_ledger_engine_test.rs` (plus their 2 "could not compile" lines). Both files contain
  **zero** occurrences of `WorkspacePolicy` or `secret_deny`, against a control of 4 occurrences in
  a file I did touch. Out of scope per the scope boundary; left red rather than papered over.
- The full `wcore-agent` suite was NOT run to completion, because the crate's test targets do not
  all compile under `-D warnings` for the reason above. Only the directly-affected target was run.

## 7. Both-directions proofs

Every gate was driven red by mutation and green by restoring. Worktree verified clean after
(`git diff` = 0 bytes).

| Mutation | Gate | Mutated | Restored |
|---|---|---|---|
| A: reintroduce the eager boot walk in `contained()` | `contained_construction_does_not_walk_the_workspace` | **FAILED** (0 passed, 1 failed) | ok, 1 passed |
| B1: `.standard_filters(true).require_git(false)` (honor `.gitignore`) | `dynamic_deny_ignores_gitignore_for_secrets` | **FAILED** | ok, 1 passed |
| B2: `.filter_entry(name != "node_modules")` (prune) | `contained_denies_secret_inside_machine_dirs` | **FAILED** | ok, 1 passed |

B1/B2 are the security invariant: **a planted secret under a gitignored / machine-named directory
is still denied.** Both fire.

**My first attempt at mutation B was a dud and I nearly recorded it as a result.** I set
`standard_filters(true)` alone and both security tests stayed green. That is not a dead gate —
`ignore` only applies `.gitignore` inside a git repository, and the tempdirs have no `.git`, so the
mutation changed nothing. Adding `.require_git(false)` made it bite immediately. **An ineffective
mutation is indistinguishable from a gate that cannot fail**, and the only thing that separated
them was going back and checking why the mutation had no effect.

## 8. Instrument defects found — all repaired in-lane, not just noted

1. **`wc -l` fabricated `0` for a 12-line file** on the lane's very first measurement. Every count
   in this report is read from a file with the Read tool.
2. **zsh ate an unquoted `--include=*.rs`**. All globs quoted.
3. **`--trust-workspace --version` silently never grants** — `--version` returns before the grant
   at `main.rs:1811`. My first posture differential was therefore two identical *untrusted* runs
   presented as trusted-vs-untrusted, and its "no difference" result was meaningless. Caught by
   checking for `workspace-trust.json`, which did not exist. Repaired: the grant is now asserted by
   the store file *and* the `Trusted workspace executable fingerprint` line before any differential
   is believed. This is the "a participant never started" self-pass.
4. **`${PIPESTATUS[0]}` under `sh`** — my gate script died with `Bad substitution` at line 3, so
   clippy never ran while the log still looked like a completed check. Repaired: `bash` plus
   `set -o pipefail`, and rc-critical commands redirect to files instead of piping.
5. **`strace -k` cannot reach the walk** (1,429 `openat` in 120 s). Replaced with in-process
   `Backtrace::force_capture()` plus PID correlation.
6. **The paint-detection timing harness never triggered** and emitted nonsense negative durations
   from an unset variable. Root-caused (the TUI does not paint headless) and the metric was
   withdrawn rather than reported.

## 9. Left open — NOT fixed, for the orchestrator

- **HIGH-ish, and now the dominant cost: the per-Bash-exec re-walk.** `secret_deny_paths_dynamic()`
  re-walks the entire workspace with no prune on **every single Bash command** in a Contained or
  Full posture. On the tree where the boot walk cost 294–367 ms, every Bash command pays that
  again. This is where the brief's "cache keyed on a cheap directory fingerprint" idea actually
  belongs — I aimed it at the boot walk and found the boot walk was simply dead. Pre-existing
  (#234), out of this lane's scope, and worth its own lane.
- **The recursive watcher adds one inotify watch per directory** and does not prune. On a
  2.5M-entry tree that is on the order of 470k watches, which will exceed
  `fs.inotify.max_user_watches`; the failure is swallowed as a `tracing::warn!` and external-edit
  tracking silently degrades. Related to the EMFILE/inotify contention already noted in the lane
  brief.
- **Posture dependence is real and undocumented in the brief:** the boot walk only ever ran in
  `Contained`, i.e. the first run in a not-yet-trusted directory. A fingerprint-trusted local
  session never paid it. That made the cost a first-impression cost specifically.

## 10. What I did NOT do

No PR, no merge to integration, no tag, no GitHub issue touched, no `wcore-contract generate`.
No `git rebase`, `reset`, `stash`, or `clean` — the only checkout used was
`git checkout -- <one path>` to revert mutations on hetzner. Pushed `lane/boot-walk` only.
No credential was used, printed, or written anywhere; nothing in this lane needed one.
No test was weakened, ignored, deleted to reach green, or re-gated; the one red gate
(`clippy -p wcore-agent`) is reported red with its cause.
