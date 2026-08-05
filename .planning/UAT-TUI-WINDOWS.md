# UAT — TUI and CLI as a product, on real Windows

Lane `uat-tui-windows`. Every claim below traces to an invocation of a real release binary on
real Windows hardware, with the transcript captured under
`.planning/evidence/uat-tui-windows/captures/`.

**Verdict: the product works on Windows.** The TUI renders correctly, a real provider turn
streams and answers, the approval flow gates both ways, the process exits cleanly with no
orphans, and paths with spaces and paths past `MAX_PATH` all work. Five defects found, all
cosmetic-to-moderate; **none blocking**. The most user-damaging one is that the product's own
error message tells you to do something that cannot work.

---

## 1. The binary — verified before any conclusion was drawn from it

Built from source **on the Windows host itself**, so the commit is not in question.

| Field | Value |
|---|---|
| Source commit | `e9bed1af931f02aea094469d44eed291af0c4c96` |
| Commit provenance | asserted identical from **three** independent places: my Mac worktree `rev-parse`, `git ls-remote` to GitHub from the Mac, and the Windows clone's own `rev-parse` after checkout |
| Path | `D:\lane-uat-tui-win\target\release\wayland-core.exe` |
| Size | 99,394,048 bytes |
| SHA256 | `9BB95A2673A7872CD87A24609FF988B6F9F68AAA74AA985018DE564F1DEEB28D` |
| PE header | `MZ` … `50450000`, machine `0x8664` = **x86_64 (AMD64)** — read out of the PE header directly, not inferred from the filename |
| `--version` | `wayland-core 0.12.25` |
| Build | `cargo build --release -p wcore-cli`, rustc 1.95.0, exit `0` in ~17 min |

**Host:** `SeanDesktop`, Windows 11 Pro build 26200, i9-13900KF (32 logical), 127.8 GB RAM,
PowerShell 5.1.26100.8875. Work confined to `D:\lane-uat-tui-win\`.

**The exists-and-is-executable gate was proven to fire.** Pointed at
`…\NOT-THE-BINARY.exe` the same gate emits `IDENTITY_ABORT: binary absent or not readable`
and exits 9. A gate that cannot fail would have certified anything.

---

## 2. What the user sees — the TUI does render

Driven through a **real ConPTY** (`pywinpty`, the same Win32 pseudoconsole API Windows
Terminal itself uses), rendered back through `pyte` so the capture is the actual character
grid a user would see at 120×40.

This mattered: the repo's own `pty_capture.rs` is `#![cfg(unix)]` because `portable_pty`'s
ConPTY backend does not surface child stdout, so the existing harness **cannot** drive a
Windows TUI at all.

Cold start, no provider configured (`captures/tui_cold.settled.screen`):

```
  ◆ WAYLAND   Workspace   Sub-Agents   Plan   Config   Diagnostics   Workflows
────────────────────────────────────────────────────────────────────────────────────────
                    ┌ Connect a provider ─────────────────────────────────────────────┐
                    │      __      __  _____ _____.___.____       _____    _______     │
                    │     /  \    /  \/  _  \\__  |   |    |     /  _  \   \      \    │
                    │                            the autonomous AI agent              │
                    │                          connect a provider to begin            │
                    │Paste your provider API key — we'll detect which provider.       │
                    │┌───────────────────────────────────────────────────────────────┐│
                    ││› paste your provider API key here                             ││
                    │└───────────────────────────────────────────────────────────────┘│
                    │▸ Enter an API key — any major provider   ⏎                      │
                    │  Use Ollama — a local model, no API key needed   o              │
                    │  Skip for now (set up a provider later from /config)   s        │
                    │                            ● Connect   ─   ○ Ready              │
                    └─────────────────────────────────────────────────────────────────┘
────────────────────────────────────────────────────────────────────────────────────────
   no model │ Smart  · Default  · STRICT REPO │ ctx                    0% │ — │ 12s
```

Measured off the raw byte stream of a live session (`tui_live.raw`, 19,116 bytes):

| Property | Result |
|---|---|
| Box drawing | `─` ×410, `│` ×64, `┌`, `◆`, `—` — all correct UTF-8, nothing mojibake |
| Colour | 658 SGR sequences, 22 distinct, **152** 256-colour (`ESC[38;5;`) |
| Alt screen | entered **once**, exited **once** — the user's terminal is properly restored |
| Cursor | hide ×9 / show ×101; hidden behind the modal, visible on the input line |
| Line endings | **zero** CRLF and **zero** bare LF — the TUI positions absolutely and never emits a newline, so the classic Windows CRLF class cannot arise in TUI mode |

Onboarding is correctly skipped when a key is present.

---

## 3. A real agent turn — streams and answers

FluxRouter (`flux-standard`), key supplied **on stdin only**.

**Headless** (`captures/live1.*`): `WLRC=0`, 5,056 ms, stdout `* 391` — 17 × 23 = 391, correct.

Per the "read the arm back from the product, not from your env" rule, the provider was
confirmed from the product's *own* output — `api.fluxrouter.ai/v1` appears in its startup
lines — not inferred from what I exported.

**In the TUI** (`captures/tui_live.*`): typed, submitted, and answered `391` on screen.
Streaming is genuinely incremental — the capture grew 9,465 → **16,771** → 17,627 bytes
across the turn, and the status-bar timer advanced 5s → 19s live:

```
   ▌ What is 17 multiplied by 23? Reply with just the number.        ┌ Activity ─────────┐
   ⚙ Pricing unavailable for flux-router/flux-standard; the call     │ Pricing unavail…  │
   remains bounded by the token envelope and cost is unpriced,       │                   │
   by the token envelope and cost is unpriced, not $0.               │                   │
     391                                                             │                   │
  ────────────────────────────────────────────────────────────────────────────────────────
   flux-standard │ Smart · Default · STRICT REPO │ ctx 0% │ $0.00 + unpriced │ 19s
```

The pricing notice is a good piece of honesty — it says *unpriced, not $0*, which is exactly
the distinction a user needs.

---

## 4. The approval flow — gated in **both** directions

Driven live, with the **filesystem as the arbiter** rather than a screen string.

The dialog shows a real diff preview before you decide (`captures/tui_approve.dialog.screen`):

```
    ⬇ Write uat-approve.txt  uat-approve.txt
    + HELLO_UAT

    [enter/y] approve   [a] always for this tool   [n] deny   [esc] cancel
   [ctrl+f] expand
     ⊘ Awaiting your approval: Write
```

| Direction | Key | Outcome | Evidence |
|---|---|---|---|
| Approve | `y` | file **created**, 9 bytes, content `HELLO_UAT` | `rc=0`, `POST_APPROVE_FILE_EXISTS: True` |
| Deny | `n` | file **not created**; UI shows `⊘ Write({…}) · rejected by user` | `POST_DENY_FILE_EXISTS: False` |

The denial is a real measurement, not a free pass: the same call also asserted a
**known-positive** — `uat-approve.txt` still present → `True` — proving the `Test-Path`
instrument was alive when it reported the other file absent.

---

## 5. Exit and orphans — clean

| Check | Result |
|---|---|
| Baseline before any launch | `wayland-core` count **0** |
| First Ctrl+C | shows `Press Ctrl+C again to exit` — a deliberate double-confirm, **not** a hang |
| Second Ctrl+C | exits `rc=0` |
| After every run in this UAT | `WAYLAND_CORE_TOTAL: 0` — **no survivors** |
| Alt screen | 1 enter / 1 exit — terminal not left corrupted |
| CI runner services | all three still `Running`, untouched |

No orphaned process was observed at any point. The documented Windows reaping/stale-lease
defect class did **not** reproduce on this build.

---

## 6. Path handling — including past `MAX_PATH`

`backup create` → `verify` → `restore`, which is where the real `os error 3` defect lived.

| Case | Home path length | create | verify | restore | restored file |
|---|---|---|---|---|---|
| Spaces (`…\path tests\a home with spaces`) | 49 | `rc=0` | `rc=0` | `rc=0` | present |
| Just under MAX_PATH | 210 | `rc=0` | `rc=0` | `rc=0` | present |
| **Past MAX_PATH** | **305** (restore target **307**) | `rc=0` | `rc=0` | `rc=0` | present |

**The `os error 3` defect does not reproduce at `e9bed1af`.**

This host has `LongPathsEnabled=1`, which would normally make the >260 result untrustworthy
as a general claim. I closed that gap by reading the shipped binary's embedded manifest:
**`longPathAware` appears 0 times** (with the search proven alive on the same file — `MZ` at
offset 0, 1095 `rustc` markers, 12 `assembly`, 2 `<?xml`). Windows only lifts `MAX_PATH` for
the Win32 path APIs when the registry key **and** a `longPathAware` manifest are both
present. This binary declares neither, so the success is attributable to Rust std's internal
`\\?\` prefixing — which is registry-independent, and should therefore hold on a default box.

That is a reasoned inference from a measurement, not a direct one. See UNRUN #1.

---

## 7. Findings, ranked by user impact

### F1 — MEDIUM. The product's own error tells the user to do something that cannot work

On a non-TTY with no prompt, the product prints:

> `wayland-core: stdin is not a terminal and no prompt was given.`
> `Use --json-stream for headless/piped use, or pass a prompt with -p.`

But `-p` is **`--provider`** (`main.rs:269`, `#[arg(short, long)]` on `provider`). The prompt
is positional (`trailing_var_arg`, `main.rs:590`). A user who follows the instruction gets:

```
Error: Unknown provider: 'Reply'. Expected a built-in provider (anthropic, openai, …)
```

I hit this by literally following the product's own advice. Source of the bad string:
`crates/wcore-cli/src/main.rs:2031-2032`. Fix is one line — drop `with -p`, or say
"pass a prompt as the final argument".

### F2 — LOW/MEDIUM. The "no API key" error names the wrong environment variables

```
Error: No API key found. Provide via --api-key, config file, or environment variable
(API_KEY, ANTHROPIC_API_KEY, or OPENAI_API_KEY).
```

`FLUX_API_KEY` is absent from that list, although FluxRouter is a first-class provider
(`config.rs:2965`) and `capability_advisory.rs:40` states the resolver's *first* arm is
`FLUX_API_KEY`. A FluxRouter user is told to set a variable that will not help them.

### F3 — LOW. 37 lines of startup spew on a single trivial headless turn

One `17 × 23` question produced **29 INFO + 5 WARN** lines on stderr, including advisories
about Spotify tools, Postgres, a browser backend, TTS and transcription that the user never
invoked. stdout stays clean so piping still works, but interactive headless use is noisy.

### F4 — LOW. Headless stdout has no trailing newline

`live1.stdout` is exactly 5 bytes: `*` `space` `3` `9` `1`. The next shell prompt runs
straight into the answer.

### F5 — LOW. Headless answers carry a `* ` bullet prefix

Present even with `--no-tui` and `NO_COLOR=1`, so `-p`-style output is not directly
machine-consumable without stripping the first two characters.

### Things that are notably right

The Ctrl+C double-confirm; the approval dialog showing a real diff *before* you approve;
`⊘ … rejected by user` after a denial; the "unpriced, not $0" honesty; and
`egress security ENFORCING … allowlisted=36` at startup.

---

## 8. UNRUN cells — **2**, both with measured reasons

A skip is not a pass. Both are reported rather than inferred.

**UNRUN 1 — behaviour with `LongPathsEnabled=0`.** Not run. The key is machine-wide and this
box hosts three live self-hosted CI runner services; changing it could disrupt other people's
builds, and it is not mine to change. Partly mitigated by the manifest analysis in §6, which
argues the result is registry-independent — but that is an inference, not a measurement.

**UNRUN 2 — glyph rendering inside Windows Terminal / conhost on the interactive desktop.**
Not run. Measured reason: ssh lands in **session 0** (`SESSION_ID: 0`) with no desktop —
`CopyFromScreen` fails with `The handle is invalid`. The interactive session (1, `seand`) is
**disconnected**, and targeting it needs `schtasks /it`, which requires Sean's password.

What this does and does not leave open: the character-cell content, colour, cursor and
alt-screen behaviour **were** verified through the same ConPTY API Windows Terminal uses, so
the product's byte stream is proven correct. What is unproven is purely front-end **font
glyph fallback** — whether a given user's console font has `─ ◆ ▸ ⏎ ⊘`. That is a per-user
font setting rather than a product property, but it was not observed and is not claimed.

---

## 9. Instrument defects hit in this session — six, all repaired in-lane

Recorded because each would have produced a confident, false product finding.

1. **`rtk` rewrote `git log` on my first command** — reported top commit `9fc9a2ff` while
   `/usr/bin/git rev-parse HEAD` said `e9bed1af`; the proxy had dropped the merge commit that
   *is* HEAD. Everything load-bearing thereafter used absolute-path tools.
2. **zsh ate an unquoted `--include=*.rs`** and the known-positive control returned **0** —
   which would have "proved" `FLUX_API_KEY` absent from the codebase. Re-run quoted: the
   control returns 53 files, and `FLUX_API_KEY` is real.
3. **cmd/PowerShell quote mangling**, measured: an inline `python -c "…\"x\"…"` arrived as
   `print(" pywinpty\,` → `SyntaxError`. Every script and spec was thereafter written locally
   and `scp`'d as a file, with JSON specs so no quoting crosses the boundary.
4. **The ConPTY driver was dead three times**, each time looking like a clean run reporting a
   blank screen: (a) list argv where pywinpty wants a command string; (b) a reader thread —
   pywinpty is not thread-safe there, it died after the first chunk and the exception was
   swallowed; (c) a 2.0 s read deadline against a **measured ~3.0 s ConPTY first-output
   stall** (bytes land at 0.00s→4, 0.01s→28, then nothing until 3.03s→104). **Any Windows
   ConPTY harness with a settle under ~3.5 s silently reports an empty screen.** All three
   were caught by a known-positive control, none by inspection.
5. **ssh + PowerShell `Get-Content` mangled UTF-8 on readback.** `--help` appeared to show
   `(�%�50%` and `�?"`. I nearly filed a HIGH "Windows console mangles help text". The raw
   stream contained clean `\xe2\x80\x94` / `\xe2\x89\xa5`, decoded as valid UTF-8, and the
   capture **file** had 0 U+FFFD — the *transport* was the corruption. Captures are now
   pulled by `scp` and read locally.
6. **Process count was a worthless build-liveness signal, and cost 35 minutes.** I polled
   `Get-Process cargo,rustc` and saw a healthy 4–33 processes while **my build had never
   started** — its `target/` did not exist and its log held one line. The processes belonged
   to `C:\actions-runner-core\`, which by coincidence was running the *same*
   `cargo build --release -p wcore-cli`. Root cause: `Start-Process` from an ssh session is
   killed when that session closes. Repaired by launching via `Win32_Process.Create` and by
   polling **my own `target/` growth** instead of a global process count. This is the
   "participant that never started reports a clean run" shape, with a decoy.

One further note in the same family: `render.py`'s three-assertion self-test **correctly
refused** to certify a "repair" I had written, because assertion 3 (*the old matcher would
have missed it*) failed — the old path handled the input fine. My hypothesis was wrong and
the self-test caught it before the wrong fix shipped into the evidence.

---

## 10. Host left clean

| Item | State |
|---|---|
| `D:\lane-uat-tui-win` | **removed** — 13,567 files, 2.87 GB |
| `D:` free after | 5,412.8 GB |
| `wayland-core` processes | **0** |
| Processes referencing my directory | 0 |
| `C:\actions-runner-{core,ferrox,wayland}` | **never touched** — 3 directories intact |
| Runner services | `actions.runner.FerroxLabs-wayland.SEANDESKTOP`, `…wayland-core.ferrox-win-msvc`, `…wayland-core.SEANDESKTOP` — all **Running** |
| Nothing created at `C:\` root | confirmed |

**Credential handling.** `FLUX_API_KEY` was transferred **on stdin only**, never in `argv`,
never written to a file, never echoed (only its length, 51, was ever printed), and was
unset in a `finally` block after each run. A sweep of every captured artifact for the literal
value returns **0 files** — and the sweep instrument was proven alive on a seeded control
file in the same invocation, which matched **1**. The Windows host retained no copy; the
whole tree containing the venv, repo and captures was deleted.
