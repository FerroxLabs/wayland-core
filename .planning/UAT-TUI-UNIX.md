# UAT — wayland-core TUI + CLI on Linux and macOS

**Lane:** `uat-tui-unix` · branch `lane/uat-tui-unix` · integration base `e9bed1af`
**Date:** 2026-07-30 · **Method:** launched and drove the shipped release binary inside a real
pty. No conclusion in this document rests on a `cargo test` line.

Every transcript referenced here is committed under
`.planning/evidence/uat-tui-unix/`. Harnesses: `tui-drive.sh`, `boot-timing.sh`,
`agent-turn.sh`, `linux-turn.sh`, `slow-type.sh`, `clean-exit.sh`.

---

## 1. Binaries under test — identity

The binary self-reports its source commit via `--build-info`, so provenance is measured, not
assumed.

| | Linux | macOS |
|---|---|---|
| Path | `/root/wayland-uat-tui-unix/target/release/wayland-core` (hetzner-dsm) | CI artifact `8752771508` |
| Arch | `ELF 64-bit x86-64` | `Mach-O 64-bit executable arm64` |
| Size | 98,435,048 B | 80,209,792 B |
| sha256 | `fae1287ac591e16c4d1595e49378ae69ce4515f752c1247cb8b96e8d959d059a` | `e5803944aead3c987b8b158a71576bc2d0b49dde6e71a6463df20590089c662b` |
| `--version` | `wayland-core 0.12.25` | `wayland-core 0.12.25` |
| `--build-info` source | **`e9bed1af`** (= integration base) | **`a903142b`** |
| Built | by this lane, `cargo build --release --locked -p wcore-cli`, rc=0, 5m56s | GitHub Actions self-hosted runner |

**arm64 verified with a discriminating control.** `lipo -archs` on the artifact → `arm64`;
the same tool on `/bin/ls` → `x86_64 arm64e`. The instrument distinguishes, so the single-arch
reading is real and not a dead tool returning one value.

**macOS is 33 commits behind the integration base**, and that is disclosed rather than papered
over: `a903142b` is an ancestor of `e9bed1af` (`git merge-base --is-ancestor`, rc=0). No darwin
artifact exists at `e9bed1af` — its CI run `30532433958` is `status=pending` with **zero jobs
scheduled**. macOS results are therefore attributed to `a903142b`, never substituted with Linux.

**Provenance trap found on the way in.** The pre-existing hetzner binary the brief pointed at
(`/root/wayland/target/release/wayland-core`) has mtime `09:35:35 UTC`, while `e9bed1af` was
committed at `09:51:51 UTC` — the binary **predates its own checkout by 16 minutes** in a tree
that `git status --porcelain` reports clean. Anyone reporting "UAT at e9bed1af" from that
binary would have been stating something false with a clean tree as backing. This is why the
lane built its own.

---

## 2. Verdict

The product is **good where a user looks, and hostile where a user starts.**

The TUI itself is genuinely strong: it renders correctly on both platforms, the onboarding
screen is well designed, the tool-approval flow is one of the clearest I have driven, the
sandbox actually blocks what it claims to block, and Ctrl-C exits with zero orphans on both
platforms. None of that is in doubt.

But three defects sit directly on the first-run path, and two of them are the very first thing
a new user does. **A first-time user's opening message loses its first few words, silently.**
**A first-time user on a Linux server cannot complete a single prompt without discovering an
undocumented environment variable.** Both reproduce on both platforms.

---

## 3. Findings, ranked by what would embarrass us most in front of a first-time user

### F1 — HIGH — The TUI silently eats the beginning of your first message

Type your first message and the opening characters vanish. No error, no indication.

Reproduced at **human typing speed (7.1 chars/sec, one keystroke at a time)** on both
platforms — this is *not* an artifact of scripted input:

```
SURFACE_BEFORE_TYPING=ONBOARDING
TYPING 44 chars at 0.14s/char (~7.1 chars/sec)
SURFACE_AFTER_TYPING=CHAT
  › the bash tool to run echo SLOWTYPE_TOKEN      <- what arrived
SENT_TEXT=[Use the bash tool to run echo SLOWTYPE_TOKEN]   <- what was typed
```

`Use ` is gone. Identical result on Linux (`slow-type.sh`, both platforms).

Mechanism: the **"Connect a provider" onboarding modal is displayed even when a provider is
fully configured** — `FLUX_API_KEY` exported *and* `-p flux-router -m flux-auto` passed on the
command line, with the status bar already showing `flux-auto`. The modal absorbs the first
keystrokes, then swaps to the chat surface mid-typing and the remainder lands in the composer.

Loss is a race, so it varies:

| Sent | Landed in composer | Lost |
|---|---|---|
| `What is 17 times 23? Reply with just the number.` | `17 times 23? Reply with just the number.` | 8 chars |
| `MARKERSTART_what is two plus two_MARKEREND` | `two plus two_MARKEREND` | **20 chars** |
| `Use the bash tool to run exactly: echo HELLO_FROM_UAT` | `the bash tool to run exactly: echo HELLO_FROM_UAT` | 4 chars |
| `/quit` | *(nothing — modal never dismissed)* | **100%** |
| `ABCDEFGH_THIS_IS_THE_START_OF_MY_TYPED_LINE_1234567890` | *(nothing — modal never dismissed)* | **100%** |

The last two are the worse half of this: at 25s and 30s settle the modal was **still up** and
the input went nowhere at all. So the behaviour is not merely "loses a prefix" — sometimes the
user's entire first line disappears into a modal that does not visibly have focus.

Evidence: `m2-tui-turn.log`, `m4-repro.log`, `m3-inputdrop.log`, `m9-quit.log`,
`l3-tui-turn.log`, `mac-slowtype-after.txt`.

### F2 — HIGH — The headless CLI cannot complete one turn out of the box on a server

`wayland-core -p flux-router -m flux-auto --no-tui "<prompt>"` on a headless Linux host — the
canonical deployment — exits **rc=1** without answering:

```
error: Session persistence authority unavailable: secure recovery storage is unavailable:
no OS keyring was usable and no encrypted credentials vault is unlocked. On a headless host
set WAYLAND_VAULT_PASSPHRASE_FD … or turn durable sessions off with [session] enabled = false
```

| Config | Platform | rc | Result |
|---|---|---|---|
| pristine HOME | Linux | **1** | above error, no answer |
| pristine HOME | macOS | **1** | same error |
| real HOME (`backend="plaintext"`) | macOS | **1** | `Error: storage.credentials.backend is set to "plaintext", which cannot hold the confidential key that durable session recovery requires` |
| `+ WAYLAND_VAULT_PASSPHRASE` | Linux | **0** | `* WAYLAND_UAT_OK` `[turns: 1 \| tokens: 2563 in / 8 out]` |
| `+ WAYLAND_VAULT_PASSPHRASE` | macOS | **0** | `* WAYLAND_UAT_OK` `[turns: 1 \| tokens: 8581 in / 8 out]` |

A headless server has no OS keyring by definition, so **rc=1 is the default experience there**,
not an edge case. The message does name the remedies, which is better than nothing — but a
brand-new user's first command failing on a credential-vault concept they have not met yet is
the wrong first impression, and none of `README`-level onboarding warned them.

*Caveat, stated because it bounds the claim:* my macOS runs execute in a non-GUI shell, where
the login keychain is not reliably available. A user in Terminal.app may well have a usable
keyring and never see this on macOS. **On headless Linux there is no such caveat.**

Provider arm verified from the product's own output, not from my environment (LANE-BRIEF
§3b-ii): captures contain `flux-router/flux-auto` and 5× `api.fluxrouter.ai`, and **zero**
`api.anthropic.com`, despite `/root/.wayland/.env` existing on hetzner (the runner checks and
reports this).

### F3 — HIGH — The tool-result line is wrong three ways at once, and hides the output

After approving a shell command the user is shown:

```
● Bash({ "command": "echo HELLO_FROM_UAT" }) · done
  Ran `?` · exit 0 · 0 bytes
```

Three defects on one line: the command renders as **`?`**, the byte count is **`0`** for a
command that printed 15 bytes, and **the command's actual output is never displayed at all**
(grep for the token as standalone output: 0 occurrences; control grep for the token anywhere:
12 — instrument alive).

Worse, on a *failed* call the same line still says `exit 0`, contradicting the glyph directly
above it:

```
✗ Bash({ "command": "echo PROOF_TOKEN_9F3A > /private/tmp/uat-…) · error
  Ran `?` · exit 0 · 0 bytes
```

A user reading `exit 0` concludes it worked. It did not. Identical on Linux
(`l3-tui-turn.log:227`). The string `Ran \`?\` · exit 0 · 0 bytes` was byte-identical in all
4 observed occurrences across both platforms and across both success and failure.

*The point of approving a shell command is to see what it did.* Right now you approve it and
learn nothing.

### F4 — MEDIUM — A wall of engine internals on every headless run

One-line prompt → **32–68 lines** of INFO logging before any answer: egress policy, browser
backend probe, postgres, seven separate `spotify_*` tool-registration lines, image/vision/
transcription/tts backend selection, cron wiring, plugin name collision, semantic-search
degradation, memory decay scheduler, channel lease **including pid and absolute paths**.

This is engine debug output on the default path, and it buries the actual answer.

### F5 — MEDIUM — `--help` is 216 lines and leaks internal ticket IDs at users

Real user-facing strings in `--help`: `F-089: model catalog commands`,
`v0.6.4 Task 2.4: serve the engine's tool registry…`, `F23-02 (Phase 23B)`, `W9.1 T4 (T11)`,
`F24-B`, `F25-03`, and an entire paragraph beginning
`23A-C1: RE-ADVERTISED because governed promotion now exists.` followed by internal
justification about a prior `bail!`. `-h` is still 142 lines. Linux 215 / macOS 216 lines.

Our sprint numbering is not a feature description.

### F6 — MEDIUM — The "no API key" error names 3 of the ~10 supported providers

```
Error: No API key found. Provide via --api-key, config file, or environment variable
(API_KEY, ANTHROPIC_API_KEY, or OPENAI_API_KEY).
```

The product supports `FLUX_API_KEY`, `GEMINI_API_KEY`, `GROQ_API_KEY`, `OPENROUTER_API_KEY`,
`DEEPSEEK_API_KEY`, `XAI_API_KEY`, `MOONSHOT_API_KEY`, Ollama and more — and the TUI onboarding
one surface away advertises *"Enter an API key — any major provider"*. A user holding a Gemini
key is told, by the product, that their key is not one of the options.

This is the same defect family the codebase already documented and fixed once for image
generation (`capability_advisory.rs:40`) — the hint sent users to keys they did not have.

### F7 — MEDIUM — Linux TUI takes 11s to first paint; macOS takes 2s

Measured at 1s granularity, instrument alive on both:

| | splash | onboarding visible |
|---|---|---|
| macOS | 1s | **2s** |
| Linux | 1s | **11s** |

11 seconds of `⠋ starting engine · connecting tools & MCP servers…` on bare-metal hardware.
*Caveat: hetzner-dsm is shared with other lanes; some of this is contention.*

### F8 — LOW — The answer and a debug log get concatenated onto one line

Headless Linux, verbatim:

```
* WAYLAND_UAT_OK[2m2026-07-30T10:21:46.964682Z[0m [32m INFO[0m auto-memorize skipped this session…
```

The assistant's answer has no trailing newline before the next log line.

### F9 — LOW — `--doctor` grades the same missing dependency differently per platform

Missing chromium is `[FAIL]` on Linux (**true rc=1**, 3 FAIL / 3 WARN / 2 PASS) but `[WARN]` on
macOS (**true rc=0**, 0 FAIL / 2 WARN / 2 PASS). Defensible — Linux CUA needs it — but a Linux
operator wiring `--doctor` into a health check gets a red for optional browser tooling.

*(Exit codes here were re-measured without a pipe. `$B --doctor | head -20` reported `RC=0`
because the pipe returns `head`'s status — the trap the lane brief names.)*

### F10 — LOW — macOS binary is adhoc/linker-signed with no Developer ID

`codesign -dv` → `Signature=adhoc`, `TeamIdentifier=not set`. A user downloading this from a
release page in a browser gets a Gatekeeper block. (My copy came via `gh api`, so it carried
only `com.apple.provenance` and ran — a browser download would not.)

---

## 4. What worked well — stated because it is the majority of the product

- **The TUI renders correctly on both platforms.** Layout, ASCII logo, activity rail, status
  bar with model / context% / cost / elapsed. No corruption at 120x40.
- **The onboarding screen is well designed** — three clear routes (paste key / Ollama local /
  skip), and it detects the provider from the key rather than asking.
- **The approval flow is excellent.** `❯ Run a shell command`, the command on its own line, and
  `[enter/y] approve once   [a] always for <cmd>   [n] deny   [esc] cancel`, echoed in the
  bottom bar *and* the activity rail (`⊘ Pending(1) press y`). Unambiguous.
- **Denial is clean and correctly worded**: `⊘ Bash({…}) · rejected by user` and
  `Tool denied: User declined — explain what to change instead`. The command renders correctly
  here (unlike F3). Verified on both platforms.
- **The sandbox actually works.** A write to `/private/tmp` outside the workspace was blocked,
  and the model explained why: *"the path is not writable from the sandbox … the sandbox
  restricts writes outside of allowed locations."* I nearly logged a false HIGH here before
  reading the frame.
- **Clean exit, zero orphans, both platforms.** Ctrl-C → child-written `WLRC=0`, and the
  argv0-exact process scan returned **1 while running / 0 after exit**, with the live scan as
  its own known-positive. Zero orphaned direct children.
- **Streaming works** — visible spinner with elapsed time and token counters
  (`✷ Steeping the bytes… (2s · ↑ 0 tokens · thinking)`), then the answer. `17 × 23` → `391`.
- **`--doctor` is platform-aware** and gives per-distro install hints; Linux-only checks are
  `[SKIP]`ped on macOS rather than failed.
- **`--build-info` prints the embedded source SHA.** This is the single most useful thing for
  attribution and it saved this lane real work.
- **No credential ever leaked into output.** Every capture swept: 0 hits, with the sweep proven
  alive on a known-positive (1 hit in a file that really contained the value).

---

## 5. Coverage — what was run, and what was NOT

Run on both platforms: `--version`, `--help`, `-h`, `--config-path`, `--build-info`,
`--doctor`, TUI first-run render, boot timing, headless turn (with and without vault),
TUI live turn, tool approval, tool denial, Ctrl-C exit + orphan sweep, human-speed input test.

**UNRUN cells — counted, not hidden (11):**

1. **macOS at `e9bed1af`** — no darwin artifact exists at the base commit (CI run pending,
   0 jobs). macOS results are at `a903142b`, 33 commits behind.
2. **macOS with a usable login keychain** (GUI Terminal session) — F2's macOS half is therefore
   bounded; the Linux half is not.
3. **Real browser-download Gatekeeper path** — my artifact came via `gh api`, so the
   quarantine flag was never set.
4. `--json-stream` protocol mode.
5. The Ollama onboarding route and the "Skip for now" route.
6. The `/config`, `/model`, `/connect` in-TUI surfaces, and the Workspace / Sub-Agents / Plan /
   Diagnostics / Workflows tabs (only the default tab was driven).
7. `wayland-core setup` and `auth add/list/login`.
8. Any of the 30 subcommands (`session`, `index`, `cache`, `workflow`, `crucible`, `forge`,
   `gateway`, `node`, `goal`, `channel`, `profile`, `backup`, `sandbox`, …).
9. Terminal sizes other than 120x40; no resize testing.
10. Multi-turn conversation, context growth, compaction (`ctx` stayed at 0%).
11. A second provider (only FluxRouter was exercised).

**A skip is not a pass.** None of the above is claimed as working.

---

## 6. Instrument defects found in my own harness, and repaired in-lane

Recorded because a harness that lies is worse than no harness (LANE-BRIEF §6b-ii — a
documented-but-unfixed instrument defect recurred on this program).

1. **pty probe measured its own plumbing.** It redirected the probe's stdout to a file inside
   tmux, so `isatty` was False *inside a real pty*. It **refused to judge the TUI** rather than
   passing — the harness working as designed. Repaired to read the pane via `capture-pane`.
2. **Orphan detector, twice wrong.** `pgrep -f <basename>` matched **17** unrelated processes
   (the checkout is named `waylandcore`); `pgrep -f <abspath>` then matched **the harness
   itself**, because `tui-drive.sh --bin <abspath>` puts the path in its own argv — reporting a
   false orphan for a PID already gone. Repaired to exact `argv[0]` match.
3. **`grep -c . file || echo 0` produced the literal `"0\n0"`** — the exact trap named in the
   brief. Replaced with `awk 'END{print NR+0}'`.
4. **`pgrep -a` is GNU-only and silently ignored on BSD**, so macOS captures held bare PIDs
   with no command text — a detector that appears to report commands and does not.
5. **macOS `env` parses options before operands.** `env HOME=x -u KEY` made `-u` the command:
   `env: -u: No such file or directory`, rc=127. GNU env tolerates the order; BSD does not.
6. **`--with-flux` referenced `$HOME_REAL` before it was set**, so the source path expanded to
   `/.wayland-secrets/flux.env` and silently sourced nothing — a credential setup failing *open*
   into an unauthenticated run that would still have looked configured.
7. **`--scrub-keys` would have unset the key `--with-flux` had just loaded**, because `env -u`
   runs after the prelude. Guarded.
8. **Pipe stole an exit status**: `--doctor | head -20` reported `RC=0` for a command whose true
   rc is 1. Re-measured unpiped.
9. **zsh ate an unquoted glob** (`crates/.../slash*.rs` → "no matches found"), killing a
   known-positive control — which is precisely why the control was there.

**Gates were proven in both directions.** `tui-drive.sh --selftest` shows the binary assertion
failing three distinct ways (missing → 92, directory → 93, non-executable → 94) *and* passing on
a real binary — run on both platforms, 4/4 both times. The pty control discriminates
(False outside / True inside). The orphan scan's known-positive is the live pre-kill scan. The
secret sweep's known-positive is a file that really contains the value.

---

## 7. Reproduction

```bash
# Linux (on hetzner-dsm)
/root/uat-tui-drive.sh --selftest
/root/uat-boot-timing.sh <bin> /root/uat-boot-home /root/uat-boot-out 90
awk -F= '/FLUX_API_KEY/{sub(/^[^=]*=/,"");print}' ~/.wayland-secrets/flux.env \
  | ssh hetzner-dsm '/root/uat-slow-type.sh <bin> <home> <out> "Use the bash tool to run echo X" 30 0.14'

# macOS (locally — the Mac is not an ssh host)
.planning/evidence/uat-tui-unix/tui-drive.sh --bin <artifact> --secrets ~/.wayland-secrets/flux.env \
  --with-flux --scrub-keys --out <dir> --label demo --home <home> --settle 25 \
  --arg -p --arg flux-router --arg -m --arg flux-auto \
  --send "__LITERAL:Use the bash tool to run exactly: echo HELLO" --send Enter \
  --send __SLEEP:12 --send y --send __SLEEP:15
```

Credential handling: the FluxRouter key was injected **on stdin only** for every hetzner run,
never in argv, never written to disk, never echoed. Final sweep across every committed evidence
file and the notes: **0 files contain the key**, with the sweep proven alive (known-positive
returns 1). The vault passphrase used is a throwaway literal
(`uat-throwaway-not-a-real-secret`), not a real secret, and is disclosed here deliberately.
