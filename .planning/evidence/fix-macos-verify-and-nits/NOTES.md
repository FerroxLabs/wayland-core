# NOTES — lane/fix-macos-verify-and-nits

Base: `e7bc6d883027102ff1e5bbaa2dd19f9265268cab` (integration `plan/f20-unified-audit-repair`).
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-fix-macos-verify-and-nits`.
Asserted at start: `git rev-parse HEAD` == base, `--show-toplevel` == the lane path.

Three tasks:
1. macOS verification of the three TUI fixes merged in `e7bc6d88` (Linux-only evidence today).
2. UAT-W1 — the non-TTY error advises `-p`, which is `--provider`.
3. Bare-letter shortcuts on a prose surface (residual from the merged TUI lane).

## T+0 — premise verification

### Task 1 premise: is a darwin binary obtainable at integration head?

**YES, and the brief's framing ("trigger a CI build and wait") understates what already
exists.** `.github/workflows/ci.yml:960` defines `build-darwin-selfhosted`:

```
name: Build (aarch64-apple-darwin) [self-hosted]
if: github.event_name == 'push' && startsWith(github.ref_name, 'lane/') && !contains(... '[ci-darwin]')
runs-on: [self-hosted, macOS, ARM64]
```

It compiles `-p wcore-cli --release --target aarch64-apple-darwin` and uploads artifact
`wayland-core-aarch64-apple-darwin` (ci.yml:1104-1111). So a **`lane/**` push is itself the
trigger** — no `workflow_dispatch` needed. Median ~16.5 min.

Two properties that constrain the plan:

- `concurrency: group: darwin-selfhosted-${{ github.ref }}, cancel-in-progress: true`
  (ci.yml:93-95). **One shot per push** — a second push cancels the first build. So the
  verification binary must be produced by a push I then do NOT supersede until it is
  downloaded.
- The job is `push`-only and `lane/**`-only, deliberately, because the runner is Sean's own
  desktop (ci.yml:56-78). The runner label `sean-mac-arm64` is **this machine** — consistent
  with LANE-BRIEF's "the Mac is NOT an ssh host, you are already on it".

Run `30546738547` exists for the base SHA itself but is on branch
`plan/f20-unified-audit-repair`, which is excluded from this job by gate 2 (integration keeps
the hermetic hosted runner). So the base SHA's own run will NOT produce an arm64 artifact.

**Plan:** push a docs-only commit first, so the built binary's `crates/` tree is byte-identical
to integration head. Assert that identity with `git diff e7bc6d88 <lane-sha> -- crates/`
being empty, and report the binary as "integration head's code, built from a lane commit that
adds only documentation" — not as a bare claim of head coverage.

### Task 2 premise: CONFIRMED at base

`crates/wcore-cli/src/main.rs:2032`:

```rust
"wayland-core: stdin is not a terminal and no prompt was given.\n\
 Use --json-stream for headless/piped use, or pass a prompt with -p."
```

`crates/wcore-cli/src/main.rs:268-269`:

```rust
/// Provider: "anthropic" or "openai"
#[arg(short, long, env = "PROVIDER")]
provider: Option<String>,
```

`short` with no value derives the first letter of the long name, so `-p` == `--provider`.
The advice is wrong. Line numbers in the orchestrator brief (2032 / 269) both HELD at base —
noted because LANE-BRIEF §"your brief's measurements are probably stale" says to expect
otherwise, and here they did not decay.

Still to establish: what the *correct* advice is (how a prompt is actually passed).

Answered: `prompt: Vec<String>` with `#[arg(trailing_var_arg = true)]` — a bare
POSITIONAL. So no flag can ever be the right advice; the correct form is
`wayland-core "your prompt"`. Fixed, with a test that resolves every flag token
in the message against the real clap `Command`.

## T+1h — Task 3 decided by 4-way cross-audit

All three external legs answered with bytes (codex 7283, gemini 19328, kimi 2121).

| leg | POSITION | SPACE | DIGITS |
|---|---|---|---|
| codex gpt-5.6-sol | B (Ctrl-K prefix mode) | UNBIND | REMOVE |
| gemini 3.1 pro | A (Alt chords) | UNBIND | MODIFIER |
| kimi K3 | C (arrows+Enter only) | UNBIND | REMOVE |

**Unanimous: UNBIND SPACE. Unanimous: nobody voted D (status quo).** Split 3 ways
on the letter mechanism, and each mechanism was refuted:

- **A refuted by its own proponent.** Gemini named the strongest objection to its
  own choice: macOS terminals map Option/Alt to dead keys, so `Alt+s` types `ß`
  unless the user has configured "Use Option as Meta". Kimi independently raised
  the same objection. This lane is the macOS lane; a mechanism that is unreliable
  on the platform under verification is disqualified.
- **B** keeps everything reachable but adds a modal command mode — new state, new
  render, new tests — for a residual narrower than the machinery.
- **C refuted on a premise I supplied.** My panel prompt asserted "every listed
  option is already a selectable row with a visible cursor". **That is false.**
  `move_cursor` walks `Path::ALL` (ApiKey/Ollama/Skip) only; the *detected env
  keys* are NOT navigable rows, so digits and `a` are the ONLY way to reach them.
  Removing the letter/digit accelerators would remove capability, which my brief
  forbids. Disclosed because the panel answered the question I asked, and I asked
  it with a false fact in it.

**Internal adversarial pass** (arguing against the emerging "just unbind space"):
the accelerators are not merely character-eaters, they have SIDE EFFECTS the
type-ahead cannot undo. `connect_env_key` and `connect_all_env_keys` both call
`persist_env_provider_selection` — they **write config.toml**. The Ready step's
Overwrite arm calls `write_config(true)`. So a bound SPACE let a prose character
commit a config write. That strengthens the unbind rather than opposing it, and
it is why the unbind is the right first move rather than a cosmetic one.

**Decided: unbind SPACE on every step; leave the letters bare.** Enter remains
the activate key, so the shortcut stays reachable and provable. This closes the
exact measured residual, because the second of the two characters was nearly
always the space.

## T+1h30 — Task 3 measured, both directions

`cargo test -p wcore-cli --lib tui::surfaces::onboarding` on hetzner:
**68 passed; 0 failed; 0 ignored; 1852 filtered out**, WLRC=0. Executed count read
back deliberately.

**The tests can fail** (LANE-BRIEF §3.2). Mutation: SPACE put back as an
accelerator on the plain Ready step (2 edits, both applied — asserted, and a
first attempt that produced a COMPILE error instead of a test failure was
discarded as proving nothing). Result: **66 passed; 2 failed**, and the failure
message is the original defect verbatim —

```
assertion `left == right` failed: the leading `s ` must survive
  left: Some(" hello")
```

### A correction, and a new finding, from a test I got wrong

I asserted `a hello` survives intact. It does NOT, and the code is right:
`a` → `connect_all_env_keys` → `Step::Name`, which HAS a focused text field, so
the type-ahead stays out of the way and the rest of the sentence is typed into
the "what should I call you" input. Assertion corrected to the measured
behaviour and pinned by `a_connect_all_diverts_prose_into_the_name_field`.

That path also WRITES config.toml. Recorded as a finding, not fixed here —
closing it needs an env-key cursor or a conditional write, both larger UX
changes wanting their own cross-audit.

## T+1h — Task 1 harness: the Linux harness is a FALSE-RED generator on macOS

Reused `.planning/evidence/fix-tui-first-message/ftfm-type.sh` as instructed, and
found a portability defect **before first use**, which is repaired in-lane rather
than written up and left (LANE-BRIEF §6b-ii):

`ftfm-type.sh` scrapes the composer with
`awk '/\xe2\x80\xba/ { sub(/^.*\xe2\x80\xba ?/, ""); ... }'`. macOS ships
onetrue awk ("awk version 20200816"), which does **not** interpret `\xNN` escapes
in a regex. Measured on this Mac against a line that plainly contains the marker:

```
printf '  \xe2\x80\xba hello world\n' | <that awk>   ->   ""      (empty)
```

The `grep -q` guard above it DOES match, so the harness takes the
`COMPOSER_PRESENT=YES` branch, scrapes nothing, and grades **TOTAL_LOSS**.
Running the Linux harness unmodified on the Mac would have reported all three
merged fixes BROKEN — a false red, on the exact question this lane exists to
answer.

Repaired in `mac-type.sh` with `index()/substr` and the marker passed via `-v`,
verified working on **both** awks (macOS 20200816 and hetzner's).

Its self-test also only covered `compute`, the pure-bash grader — so it went
green on a platform where the extractor was dead. The judgement is
`grader(extractor(pane))` and only the grader was tested. Self-test extended from
10 to **15** assertions, now covering the extractor against REAL captured panes
from the previous lane (known-positive: a pane with a composer yields its exact
text; known-negative: a composer-less pane yields nothing; plus both end-to-end
compositions). `SELFTEST=PASS assertions=15`, rc=0, and B3 reports
`branch=OLD_MATCHER_BLIND_HERE old=[]` — the platform proof.

**The self-test can fail:** two mutations (extractor always returns the text;
extractor reverted to the original awk) both give rc=**91**, clean gives rc=**0**.

## Status log

- T+0 pushing this file to start the darwin build clock. Nothing else committed yet.
- T+1h Tasks 2 and 3 committed (f95de109, c19a96f4). Not pushed — a second push
  would cancel the in-flight darwin build (`cancel-in-progress` per ref).
  Carried to hetzner by patch instead; tree hash asserted identical
  (`cb778546…` on both hosts).
- T+1h45 darwin build in progress (started 14:21:08Z). Gate suite running.
