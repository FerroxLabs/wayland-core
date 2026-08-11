# INV-2 round 4

Worktree `/root/wt-inv2r2`, branch `fix/inv2-round4` @ **`eb9fcf7e`**, off round 3 `0f4f3549`.
Bundle: `/root/lane-bundles/fix-inv2-round4.bundle`.
All builds/tests on `ssh hetzner-dsm`; `export PATH=$HOME/.cargo/bin:$PATH`
first — cargo is not on a non-interactive PATH. Never build on the Mac.

## The WIP checkpoint `9a9b89c5`

824 lines of unreviewed, unbuilt round-4 work recovered from a stopped agent.
Reviewed, built, run: **it compiles and its 93 tests pass**. Kept whole; five
substantive changes, all sound:

1. `NoRepo` is no longer memoized (R1) — only `Baseline::Repo` is cached.
2. Agent attribution is a count, not a set (F1).
3. The armD `Store::Foreign` rule — a repository recording nothing under the
   file's directory is not its archive.
4. The ambient git environment is cleared (`GIT_DIR`, `GIT_OBJECT_DIRECTORY`
   + 4 more), with a positive-controlled binary.
5. The disposal sentence rewritten around cruft packs and `gc --prune=now`.

**Nothing discarded.** Three things it got wrong or did not finish are fixed on
top; each was reproduced RED first.

## Round-4 commits

| commit | what |
|---|---|
| `1280ba25` | the note named `<root>/.git/objects` — false in a linked worktree (`.git` is a FILE there). Reproduced RED. Resolved from `git rev-parse --path-format=absolute --git-path objects`. Plus every travel claim executed against git. |
| `c6c83dc5` | a gitignored file's bytes were filed into `.git/objects`. Reproduced RED with a canary key found by `git cat-file --batch-all-objects`. Now refused. |
| `31995958` | agent attribution went stale the moment the user edited the file. Reproduced RED. Now expires. |
| `21609143` | five more git failure modes + a `git`-exits-7 shim binary. |
| `1e4b502c` | the three mutation survivors, killed. |
| `95a469ca` | armD driven end-to-end through `WriteTool::execute`. |
| `08492242` | unix gating for the Windows build. |
| `eb9fcf7e` | the last unexercised sentence in the note (`gc --auto`), executed. |

## Gates

* `cargo fmt --all --check` RC=0
* `cargo clippy --workspace --all-targets -- -D warnings` RC=0
* `cargo clippy -p wcore-tools --all-targets --target x86_64-pc-windows-gnu -- -D warnings` RC=0
* `cargo nextest run -p wcore-tools` — **1484/1484 pass**
* Mutations: **27/27 KILLED**, 0 survivors, 0 harness failures, tree restored
  byte-for-byte (`/root/inv2r4-mutate.py`, log `/root/inv2r4-mutate-final.log`)

## Measured git facts (2.43.0 Linux, `/root/inv2r4-measure.sh`)

* `git gc` ×6 leaves the copy readable (cruft pack present). `git gc --auto`
  does not fire at all at 4 loose objects — no pack written, still readable,
  on both sides of the prune window.
* `git gc --prune=now` disposes of it. An ordinary `git gc` disposes of it once
  the loose object's mtime is 3 weeks old — the `gc.pruneExpire` two-week
  default, measured with a fresh object as the in-run control.
* Carries the bytes: `cp -a`, `tar`, `rsync`, `git clone <path>`,
  `git clone --no-hardlinks <path>`. `git fsck --lost-found` writes it out as a
  **plaintext file**.
* Does NOT carry: `git clone file://`, `git bundle --all`, `git push`.
* Linked worktree: `.git` is a FILE; objects live in the main repository.
* `git check-ignore` **rejects** `--literal-pathspecs` (exit 128) — it takes
  its path on `--stdin -z`. It consults the index, so a tracked file matched by
  an ignore rule exits 1.

## Declared, not hidden

* **The recovery copy is not scrubbed and cannot be.** A copy with the secret
  redacted is not a recovery copy. Placement is the only lever: never into a
  repository that records nothing under the file's directory, never into one
  configured to ignore the file. Stated in the module docs and `docs/tools.md`.
* **The pin-write mutation `or_insert_with` -> `insert` is an equivalent
  mutant.** `resolve_baseline` reads the pin map and returns early before that
  line can run. The two differ only under concurrent resolution of the same
  root by two threads of the shared guard, which no deterministic test can
  trigger. `or_insert_with` is kept as the safer of the two; the property is
  verified instead by `P2c-out`, which attacks the read.
* **No timeout on the git spawns.** A `git` that hangs blocks the tool. It is a
  liveness bug, not a fail-open, and it is not fixed here.
* **Windows was not executed.** Both clippy targets are RC=0 and no new code is
  `#[cfg]`-ed, but no test ran on a Windows host in this round.
* **No live LLM session was run.** armD is exercised end-to-end through the
  real `WriteTool::execute` and the real object database, not through a model.
