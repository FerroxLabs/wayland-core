# 20A-03 — End-of-Line Reconciliation: Determination and Decision

**Determination: REFUTED — NO DEFECT.**

The plan's premise — that the landing reads *every normal Windows checkout* as
dirty, and that this therefore "fires for ANY real user, not only for fixtures"
— is refuted by measurement. Production never produces the checkout shape that
goes dirty, because production mints the integration checkout through the *same*
scrub that later judges it. The reported failure is produced solely by the test
fixture, which mints its integration checkout through a plain, unscrubbed
`git clone` that no production path uses.

Per the plan's own termination rule, the Task 2 blocking checkpoint **does not
run**, Task 3 **does not run**, and **no code change is made**.

- Measured on: `SEANDESKTOP`, `C:\ferrox-win` @ `c252d01d`, `git version 2.54.0.windows.1`
- All determination work ran in a scratch clone at `C:\eol-scratch`, removed afterwards.
- `C:\ferrox-win` was read only; its SHA and clean status are gate-verified below.

---

## M0 — Starting git configuration on the box, with origins

```
$ git --version
git version 2.54.0.windows.1

$ git config --show-origin --show-scope --get-all core.autocrlf
system	file:C:/Program Files/Git/etc/gitconfig	true
[exit=0]

$ git config --show-origin --show-scope --get-all core.eol
[exit=1]            # unset in every scope

$ git config --get core.autocrlf
true
```

`core.autocrlf=true` is present, and it comes from the **Git for Windows system
config**, not from anything Wayland or Sean set. `core.eol` is unset everywhere.
The brief's starting assumption about the box is correct.

---

## The single-variable measurements

Each step below changes exactly one thing from the step it is compared against.

### M1 — The fixture source repo, built exactly as `init_repo` builds it

`crates/wcore-agent/tests/transactional_delegated_mutation_test.rs:146-154`
does `git init`, then `std::fs::write(repo.join("README.md"), "base\n")`, then
`git add`, then `git commit`. Reproduced byte-for-byte:

```
-- source worktree README.md bytes (written directly by the test, LF) --
98,97,115,101,10                       # "base\n"

warning: in the working copy of 'README.md', LF will be replaced by CRLF the next time Git touches it

-- index blob bytes for README.md --
98,97,115,101,10                       # "base\n"

-- source has .gitattributes? --
NO - none present

-- source status plain --
                                        # clean
```

Git itself announces the pending smudge: *"LF will be replaced by CRLF the next
time Git touches it."* The next time Git touches it is the clone.

### M1a — Clone with **ambient** config (what the test fixture does)

`clone_integration` (line 158-176) shells out with a bare
`std::process::Command::new("git")`, so it inherits the system `autocrlf=true`.

```
$ git clone -- <src> <clone_ambient>
-- clone_ambient worktree README.md bytes --
98,97,115,101,13,10                    # "base\r\n"  <-- CRLF
```

**Variable changed vs M1: none but the checkout itself.** The clone's smudge
filter converted LF to CRLF on disk. The index still holds LF.

### M1b vs M1c — The scrub's forced value, isolated

Same directory, same bytes on disk, same index. The **only** difference between
these two commands is `core.autocrlf`:

```
$ git status --porcelain                                # ambient: autocrlf=true
                                                         # CLEAN

$ git -c core.autocrlf=false status --porcelain          # the scrub's forced value
 M README.md                                             # DIRTY
```

This is the entire symptom, isolated to one variable. With `autocrlf=true` the
clean filter normalises CRLF back to LF before comparison and the tree reads
clean; with `autocrlf=false` git compares the literal bytes and reports a
modification the user never made.

### M1d — Do attributes govern this file at all?

```
$ git check-attr text eol -- README.md      # in clone_ambient
README.md: text: unspecified
README.md: eol: unspecified
```

**Unspecified — because the fixture repo has no `.gitattributes` at all.** This
is the hinge of the whole determination: the repository that goes dirty is *not
this repository*.

### M2 — Clone with `core.autocrlf=false` (what **production** does)

`WorktreeManager::create_integration_checkout` clones through
`self.git_command(&clone_args)` (`worktree_manager.rs:1164-1180`), and
`git_command` (`worktree_cleanup.rs:414-421`) unconditionally prepends
`-c core.autocrlf=false`. So production's clone is scrubbed. Reproduced:

```
$ git -c core.autocrlf=false clone -- <src> <clone_scrubbed>
-- clone_scrubbed worktree README.md bytes --
98,97,115,101,10                       # "base\n"  <-- LF

$ git -c core.autocrlf=false status --porcelain     # the landing's own value
                                                     # CLEAN
$ git status --porcelain                             # ambient
                                                     # CLEAN
```

**Variable changed vs M1a: only the clone's `core.autocrlf`.** A
production-shaped integration checkout reads clean — under the landing's value
*and* under the ambient one.

### M3 — Does the symptom reach **the landing**, not merely a plain status?

Full replication of the scrub environment from `git_command`
(`worktree_cleanup.rs:432-457`): `GIT_CONFIG_NOSYSTEM=1`, emptied
`GIT_CONFIG_SYSTEM` / `GIT_CONFIG_GLOBAL`, `GIT_ATTR_NOSYSTEM=1`, plus the
forced `-c` arguments and `--porcelain=v1`.

```
-- TRAP CHECK (cmd `set "VAR=x"` form; no trailing space) --
[GIT_CONFIG_NOSYSTEM=1]
[GIT_ATTR_NOSYSTEM=1]
[GIT_CONFIG_GLOBAL=C:\eol-scratch\empty.gitconfig]

-- M3a: autocrlf as git resolves it under the scrub env --
$ git config --get core.autocrlf
[exit=1]                                # UNSET

-- M3b: the landing invocation, on clone_ambient --
$ git -c core.fsmonitor=false -c core.autocrlf=false status --porcelain=v1
 M README.md                            # DIRTY
```

Yes — the symptom reaches the landing's own invocation, not just a plain status.

**M3a is independently important.** Under the scrub env, `core.autocrlf` is
*unset*, because `GIT_CONFIG_NOSYSTEM=1` plus the emptied config files already
strip the user's `autocrlf=true`. Git's default for unset `autocrlf` is `false`.
So **deleting the explicit `-c core.autocrlf=false` would not change the
behaviour at all** — the rest of the hostile-config defense already produces it.
That measurement kills the naive "just drop the forced value" workaround
outright, on evidence rather than on argument.

### M4 — The landing invocation on the production-shaped checkout

```
$ git -c core.fsmonitor=false -c core.autocrlf=false status --porcelain=v1
                                        # CLEAN
```

### M5 — Is this a stale, never-renormalized checkout?

```
$ git log -1 --format=%H
aaea70c4b87de244508f3db748a496257bdb5a38     # minted seconds earlier, in this script
$ git -c core.autocrlf=false diff --stat
 README.md | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)
```

No. The dirty checkout was created seconds before the measurement, from an index
written seconds before that. It is as fresh as a checkout can be.

### M6 — Positive control on `C:\ferrox-win` under the **full scrub env**

```
[GIT_ATTR_NOSYSTEM=1]  [GIT_CONFIG_NOSYSTEM=1]

$ git rev-parse HEAD
c252d01d3c885ed97ec0eff9b04280f2e5756672

$ git -c core.autocrlf=false check-attr text eol -- README.md
README.md: text: set
README.md: eol: lf

-- README.md first 24 bytes on disk --
60,100,105,118,32,97,108,105,103,110,61,34,99,101,110,116,101,114,34,62,10,10,33,91
                                        # ...'>' 0x0A 0x0A — LF, no CR anywhere

$ git -c core.fsmonitor=false -c core.autocrlf=false status --porcelain=v1
                                        # CLEAN
```

The wayland repo's own Windows checkout resolves `eol: lf` **under
`GIT_ATTR_NOSYSTEM=1`**, holds LF bytes on disk, and reads clean through the
landing's own invocation.

### M7 — The attributes variable, isolated

Identical fixture to M1, cloned with identical ambient config. The **only**
difference is a committed `.gitattributes` containing `* text=auto eol=lf`:

```
-- clone_attr worktree README.md bytes (ambient autocrlf=true clone) --
98,97,115,101,10                        # LF   (M1a, without attributes, was CRLF)

$ git check-attr text eol -- README.md
README.md: text: auto
README.md: eol: lf

$ git -c core.fsmonitor=false -c core.autocrlf=false status --porcelain=v1
                                        # CLEAN
-- same, under full scrub env with GIT_ATTR_NOSYSTEM=1 --
                                        # CLEAN
```

One variable, opposite outcome. The in-tree `eol=lf` rule overrides
`core.autocrlf=true` at checkout exactly as the brief predicted, and survives
`GIT_ATTR_NOSYSTEM=1`.

### M8 — End-to-end confirmation, changing no code

Run the real suite on `C:\ferrox-win` with exactly one variable altered: the
ambient `core.autocrlf` that the **fixture's own unscrubbed clone** inherits,
injected via `GIT_CONFIG_COUNT`. `git_command` calls
`env_remove("GIT_CONFIG_COUNT")` (`worktree_cleanup.rs:440`), so the landing's
git provably never sees it:

```
[GIT_CONFIG_COUNT=1] [GIT_CONFIG_KEY_0=core.autocrlf] [GIT_CONFIG_VALUE_0=false]

-- override takes effect for a plain git --
$ git config --get core.autocrlf
false
-- same query with GIT_CONFIG_COUNT removed, i.e. what the scrub does --
true
```

Result: **the `parent integration checkout is dirty: M README.md` failure
disappears from all four tests.** They still fail, but on a completely different
and deeper error (see F-EOL-3 below).

---

## Determination

**REFUTED — NO DEFECT** (plan termination state 2).

The brief's *mechanical* claim about `.gitattributes` was correct and is now
measured: an in-tree `eol=lf` rule does override `core.autocrlf`, and
`GIT_ATTR_NOSYSTEM=1` does not disable it (M6b, M7a, M7d). The brief's error was
a **category error about which repository goes dirty**. The dirty checkout is
not the wayland repo and never was; it is an ephemeral temp-dir fixture repo
that has no `.gitattributes` at all.

The chain, all measured:

1. Git for Windows sets `core.autocrlf=true` in system config (M0).
2. The test fixture builds a source repo with **no `.gitattributes`** (M1, M1d).
3. The fixture clones it with a **plain, unscrubbed** `git clone`, so the
   checkout smudges to CRLF on disk against an LF index (M1a).
4. The landing judges that checkout with `core.autocrlf=false` and correctly
   reports a literal byte difference (M1c, M3b).
5. **Production never creates such a checkout.** `create_integration_checkout`
   clones through the same scrubbed `git_command` that later judges it, so the
   checkout lands LF and reads clean (M2, M4).

The scrub and the landing are therefore already self-consistent: Wayland
*guarantees* the representation it judges against rather than inheriting it.
That is precisely what the plan's `scrub-normalizes` option describes as
desirable — **and it is already implemented.** There is nothing to reconcile and
nothing to decide.

### How the alternatives were excluded

| Candidate explanation | Excluded by |
|---|---|
| (i) The checkout predates the attributes rule and was never renormalized | **M5** — the dirty checkout was minted seconds before measurement. **M6c** — `C:\ferrox-win` holds LF bytes and **M6d** reads clean. |
| (ii) The attributes file is not in effect on that path | **M1d** — it is not in effect because the fixture repo *has no attributes file*. **M6b / M7b** — wherever one exists, it resolves exactly as committed. |
| (iii) The dirt is not EOL; `M README.md` has another cause | **M1a vs M1** — byte level: worktree `98,97,115,101,13,10` vs index `98,97,115,101,10`. A single inserted `0x0D`. **M8** — neutralising only the EOL variable removes exactly that error. |
| (iv) The scrub's env disables in-tree attributes too | **M6b** — under `GIT_ATTR_NOSYSTEM=1` plus emptied system/global config, `check-attr` still returns `text: set, eol: lf`. **M7d** — the attributed clone still reads clean under the same env. `GIT_ATTR_NOSYSTEM` governs only the *system* attributes file, as documented. |

---

## Decision

**The Task 2 blocking checkpoint does not run.** The plan states: *"If Task 1
determined REFUTED-NO-DEFECT, this checkpoint does not run at all: record the
refutation, make no code change, and close."*

- `crates/wcore-swarm/src/worktree_cleanup.rs` — **unchanged.**
- `crates/wcore-swarm/src/worktree/parent.rs` — **unchanged.**
- `crates/wcore-swarm/src/worktree_tests.rs` — **unchanged.**
- `.gitattributes` — **unchanged.**

No option was selected, because the determination left none of them applicable:
`attributes-authoritative` is moot (the rule already works, M6b/M7a),
`scrub-normalizes` is already the shipped design at this surface (M2/M4), and
`relax-dirty-check` would blind a check that is currently reporting a true byte
difference in a checkout production never builds.

---

## Findings recorded, deliberately NOT fixed here

### F-EOL-1 — Test harness: the fixture mints its integration checkout unlike production

**Severity: MEDIUM (test harness, not product).**
`clone_integration` (`crates/wcore-agent/tests/transactional_delegated_mutation_test.rs:158-176`)
builds the integration checkout with a bare `std::process::Command::new("git")
clone`, inheriting ambient `core.autocrlf`. Production builds it through
`create_integration_checkout`, which clones under the scrub. The fixture's
docstring says it builds "a clean, Wayland-owned integration checkout"; on
Windows it does not achieve that, and this is the sole cause of the four
reported failures.

**Not fixed here, for two reasons.** The file is outside this plan's declared
`files_modified`, and the plan's termination rule for a fix reaching outside
those files is to stop. More substantively, the *right* fix is a real design
question — patch the clone flags, or make the fixture call
`create_integration_checkout` so it exercises the production minting path — and
the second is a non-trivial fixture redesign that should not be improvised.
Note also that fixing it would **not** turn the four tests green (see F-EOL-3).

### F-EOL-2 — Product: `assert_clean` false-positives on a user's own Windows repo

**Severity: HIGH (product, different surface, different file).**
`WorktreeManager::assert_clean` (`worktree_manager.rs:582-598`) runs the same
scrubbed `git status --porcelain` against `self.repo_root` — the **user's own
repository**, which Wayland does *not* mint and therefore cannot normalise. It
gates dispatch (`create_integration_checkout_inner`, `create_worker_tree`).

On Windows, a repository the user cloned normally, that does not commit a
normalizing `.gitattributes`, has a CRLF worktree against an LF index — and M3b
is exactly that invocation against exactly that checkout shape, returning
` M README.md`. Such a user would be refused dispatch with `DirtyCheckout` on a
pristine tree, and the message names a file they never touched.

**Not fixed here.** It is a different function in a file outside
`files_modified`, and the remedy is exactly the design decision this plan
reserves for a blocking checkpoint — every candidate value carries a cost
(`autocrlf=input`, for instance, would fix this case but newly false-positive on
any repo that legitimately commits CRLF). The plan is explicit that a silent
choice here is the failure mode to avoid, so no choice was made.

This finding does **not** reproduce on `C:\ferrox-win`, because this repo commits
the normalizing `.gitattributes` (M6). It is a defect for *other* repositories.

### F-EOL-3 — NEW BLOCKER outside this surface: `\\?\` extended-length path rejected by git

**Severity: HIGH. Reported, not opened, per the execution bounds.**
The dirty-checkout refusal was **masking** a deeper Windows defect. It fires
first, in `bind_parent_preimage` → `assert_clean_checkout`, short-circuiting
before the candidate quarantine runs. With the EOL variable neutralised (M8),
the same four tests reach the next stage and fail there:

```
candidate object graph failed quarantine revalidation:
  candidate build step ["read-tree", "cc1bdfa1894337da8480f5a64a42dbde30c80912"] failed:
  fatal: not a git repository: '\\?\C:\Users\seand\AppData\Local\Temp\.tmpBTSy4s\checkout\.git'
```

A Windows extended-length (`\\?\`) path is reaching git, which rejects it. This
is the `\\?\`-canonicalize family, not the EOL surface. Per the bounds — *"If a
NEW CRITICAL/HIGH blocker appears outside this surface, STOP and report it — do
not open a front"* — it is recorded here and left untouched.

**Consequence for expectations: there is no green available on this surface.**
Even a perfect EOL fix would leave these four tests red on `\\?\`.

---

## Known unknowns, recorded not resolved

- Whether non-NTFS or network volumes change the checkout representation.
- Whether Git for Windows versions other than 2.54.0 ship a different system
  default for `core.autocrlf`.
- Whether any other repository the swarm consumes carries attributes rules that
  conflict with this one.

---

## Gate evidence

| Gate | Result |
|---|---|
| Scratch clone removed | `SCRATCH-REMOVED` |
| `C:\ferrox-win` SHA | `c252d01d3c885ed97ec0eff9b04280f2e5756672` (pinned, unchanged) |
| `C:\ferrox-win` `git status --porcelain` | empty — unmodified |
| `cargo fmt --all -- --check` (Mac) | clean |
| `git status --porcelain -- crates/ .gitattributes` | empty — **no production file touched** |
| `git diff --exit-code -- scripts/f20-native-windows-proof.ps1` | clean — `$targets` byte-identical |
| Hetzner Linux non-regression | **not run — not required.** No production code changed. |
