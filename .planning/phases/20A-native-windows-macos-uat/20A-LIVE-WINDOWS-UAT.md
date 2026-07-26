# 20A — LIVE Windows User-Acceptance Pass

**Date:** 2026-07-26
**Host:** `seandesktop` (Windows 11 Pro, 10.0.26200), driven over SSH (OpenSSH → PowerShell 5.1)
**Binary under test:** `C:\wl-live-uat\target\release\wayland-core.exe`, 87,730,176 bytes
**Provenance (verified, not assumed):** `wayland-core 0.12.25 (source 9821ef7603ac1e687b600cda591af1657c883484)` — exact match to the sealed candidate `9821ef76` / tag `f20a-candidate-9821ef76`
**Checkout:** `C:\wl-live-uat`, detached at `9821ef7603ac1e687b600cda591af1657c883484`, `git status --porcelain` = 0 lines
**Model driving live turns:** local Ollama `qwen3:32b` via `--provider openai --base-url http://127.0.0.1:11434`

This was a live product pass, not a test-suite run. Every claim below traces to a command
that was actually executed and output that was actually observed.

---

## 0. Scope limit that shapes everything below — read first

The session driving this pass is a **non-interactive SSH logon in Windows session 0**
(`(Get-Process -Id $PID).SessionId` = `0`; the only interactive session, id 1, has been
**Disconnected** for 3+ days).

I established this by control rather than assuming it. A purpose-built Rust probe using the
**same `keyring` 3.6.3 crate and the same `windows-native` feature the product uses**,
mirroring `wcore-config::credentials::keyring_available` exactly:

```
KEYRING_AVAILABLE=false get_password_err=Couldn't access platform secure storage: Windows ERROR_NO_SUCH_LOGON_SESSION
SET_OK=false err=Couldn't access platform secure storage: Windows ERROR_NO_SUCH_LOGON_SESSION
GET_OK=false err=Couldn't access platform secure storage: Windows ERROR_NO_SUCH_LOGON_SESSION
```

Independently, the AppContainer backend reports unavailable on every launch:

```
ERROR AppContainer real-spawn probe failed; sandbox disabled.
      error=sandbox child execution failed: AppContainer ACL lease SID/profile mismatch in
      \\?\C:\Users\seand\AppData\Local\Wayland\Core\AppContainerLeases\v1\WCore-storage-00002d20-00000000000000f2.toml
```

**Consequence:** anything gated on the OS keyring or on AppContainer is **NOT VERIFIED** in
this pass, not BROKEN. I have not reported any such red as a product defect. Where a
product-side consequence was reachable *without* those gates, I reproduced it independently
and say so explicitly.

**Correction to an earlier reading of mine, recorded for honesty:** I first probed Credential
Manager with `cmdkey` and concluded it was reachable. That was wrong — `cmdkey /list:<target>`
echoes the target name even when no such credential exists, and the `cmdkey` *add* had in fact
returned exit 1. The Rust probe above is the authoritative result.

---

## Verdict summary

| Stage | Item | Verdict |
|---|---|---|
| 0 | Release build at the sealed SHA | **WORKS** |
| 1 | Binary launches; `--version` / `--help` | **WORKS** |
| 1 | Build provenance matches sealed SHA | **WORKS** |
| 1 | Config loads from real Windows path | **WORKS** |
| 1 | One-shot prompt → real model response | **WORKS** |
| 1 | Write tool — path with spaces | **WORKS** |
| 1 | Write tool — deep 220-char path | **WORKS** |
| 1 | Read / Grep / Glob tools | **BROKEN** (D1) |
| 1 | Shell / Bash tool | **NOT VERIFIED** (sandbox gated) |
| 1 | Streaming progressive, not garbled | **WORKS** |
| 1 | Ctrl+C cancels cleanly | **NOT VERIFIED** |
| 1 | No orphaned children after kill | **WORKS** |
| 2 | TUI launches | **WORKS** |
| 2 | TUI renders | **WORKS** |
| 2 | TUI keystrokes / streaming / resize / clean exit | **NOT VERIFIED** |
| 3 | Crash recovery of session journal | **BROKEN** (D2) |
| 3 | Process reaping of tool children | **NOT VERIFIED** |
| 3 | Worktree / delegation lifecycle | **NOT VERIFIED** |

---

## Stage 0 — Build

**Command:** `cargo build --release -p wcore-cli` in `C:\wl-live-uat`

**Result: WORKS.**

```
Finished `release` profile [optimized] target(s) in 7m 13s
EXIT=0
END 2026-07-26T11:37:06.9875378+07:00
```

Artifact: `C:\wl-live-uat\target\release\wayland-core.exe`, 87,730,176 bytes.
One non-blocking warning: `imap-proto v0.10.2` contains code rejected by a future rustc.

A shared-box note: a GitHub Actions `Runner.Worker` and a `cargo-nextest` job were active
concurrently on this machine during the build. No interference was observed in the result,
but compile timings here are not clean-room.

---

## Stage 1 — CLI liveness

### 1.1 Launch, version, help — WORKS

```
wayland-core 0.12.25            VERSION_EXIT=0
--help                          HELP_EXIT=0, 128 lines, 20 subcommands
```

### 1.2 Provenance — WORKS

```
wayland-core 0.12.25 (source 9821ef7603ac1e687b600cda591af1657c883484)
```

Exact match to the sealed candidate. The stale-build class is excluded.

### 1.3 Config from the real Windows path — WORKS

```
--config-path  → C:\Users\seand\AppData\Roaming\wayland-core\config.toml   (exists = True)
--skills-path  → User: C:\Users\seand\AppData\Roaming\wayland-core\skills  (exists)
```

Correct Windows Roaming-AppData shape. No Unix-shaped path. 26 config sections parsed.

### 1.4 One-shot prompt end to end — WORKS

```
EXIT=0  elapsed=78.7s
STDOUT: * PONG
raw bytes: 2a 20 50 4f 4e 47
```

A real model round-trip returning a real response. The hex confirms clean ASCII — no BOM,
no double-print, no mojibake.

Getting here required two things worth recording:
* The provider appends `/v1/chat/completions` to `--base-url`, so `--base-url .../v1`
  double-prefixes and yields `404 page not found`. Correct value is the bare origin.
* `gemma3:4b` returns `400 ... does not support tools` — Core always advertises tools, so a
  tool-capable model is mandatory even for a plain text prompt.

### 1.5 Tool calls with Windows path handling

**Write tool, path containing spaces — WORKS.** Disk-verified, not inferred:

```
prompt: create C:\wl-uat-work\dir with spaces\hello.txt containing WAYLAND_UAT_OK
EXIT=0 elapsed=104.8s
FILE EXISTS: C:\wl-uat-work\dir with spaces\hello.txt
CONTENT: WAYLAND_UAT_OK
```

**Write tool, deep 220-char path — WORKS.** Disk-verified:

```
deep dir len=214 created=True
target path len=220
EXIT=0 elapsed=142.9s
DEEP DISK EVIDENCE: DEEPOK
```

**Read / Grep / Glob — BROKEN.** See defect **D1**. The Read tool's refusal of a malformed
path did not merely fail the tool; it killed the session with exit 1.

**Shell / Bash tool — NOT VERIFIED.** It fail-closed on the unavailable sandbox:

```
> Bash({"command":"echo SHELL_RAN_OK > C:\\wl-uat-work\\shellout.txt"})
  X Refused: shell is unavailable because the active sandbox backend cannot enforce
    secret-read-deny for this workspace.
```

Fail-closed is the correct posture, and the trigger is the session-0 AppContainer limitation.
Not counted as a defect. Needs execution under the runner service or an interactive session.

### 1.6 Streaming — WORKS

Sampled the stdout file while a 600-word generation was in flight:

```
t=+120s bytes=0
t=+130s bytes=250
t=+140s bytes=511    (alive=True throughout)
```

Output grows incrementally while the process runs — genuinely progressive, not one flush at
exit. Captured text is clean, well-formed prose with no torn or duplicated segments.

### 1.7 Ctrl+C — NOT VERIFIED

I could not deliver a genuine console CTRL_C_EVENT. My ConPTY harness (below) failed its own
control, and session 0 has no console to attach to. `Stop-Process -Force` was exercised
instead and is reported under Stage 3. What is needed: an interactive session, or a working
`GenerateConsoleCtrlEvent` harness.

### 1.8 Orphans after forced termination — WORKS

```
launched pid=40928, child tree: 40664 conhost.exe
Stop-Process -Force 40928
post-kill: pid=40928 GONE, pid=40664 GONE, pid=43912 GONE
```

The entire killed process tree was reaped. One unattributed `wayland-core.exe` (pid 37932,
started 12:04:46) was observed alive during the sweep; I could not tie it to the killed tree
and do not claim it as a leak.

---

## Stage 2 — TUI liveness

**Launches: WORKS. Renders: WORKS. Everything else: NOT VERIFIED.**

### What I tried, and what blocked it

1. **Hand-written ConPTY harness** (C# P/Invoke: `CreatePseudoConsole` /
   `InitializeProcThreadAttributeList` / `UpdateProcThreadAttribute` / `CreateProcess`).
   The TUI launched and stayed alive, but captured **0 bytes**. I ran a control —
   `cmd.exe /c echo HARNESS_CONTROL_OK` under the identical harness **also produced 0 bytes**,
   as did an interactive `powershell.exe`. **The harness is at fault, not the product.**
   Most likely cause: `STARTUPINFO` declared without `CharSet=CharSet.Unicode`. I did not
   claim a TUI failure on this basis.
2. **`winpty.exe`** (present at `C:\Program Files\Git\usr\bin\winpty.exe`). Its own control
   asserted out — `ASSERT_CONDITION("wp != nullptr && cols > 0 && rows > 0")` — because
   session 0 has no console to size from. Despite that, launching the TUI through it **did**
   yield real rendered output.
3. `pywinpty` is **not installed**; I did not install packages for this pass.

### What I did observe

First frame, captured live:

```
                                  WAYLAND CORE

              ⠋  starting engine · connecting tools & MCP servers…
```

The braille spinner `⠋` and the `·` render correctly — Unicode is intact.

Full first screen (ANSI stripped) shows a complete, laid-out interface:

```
  WAYLAND   Workspace   Sub-Agents   Plan   Config   Diagnostics   Workflows
       __      __  _____ _____.___.____       _____    _______  ________
      /  \    /  \/  _  \\__  |   |    |     /  _  \   \      \ \______ \
      ...
                             the autonomous AI agent
     A terminal AI agent that reads, writes, and runs code in your project.
                   Detected: AWS Bedrock  ·  /provider to use
                           Try: explain this codebase
                 /connect to paste an API key and add a provider
  › type / for commands
  Tab next tab   ⇧Tab mode   hide rail   PgUp/PgDn scroll   End latest   F4 copy
   qwen3:32b · Smart · Default · STRICT REPO · ctx                            0%
```

Tab bar, logo, hint lines, key-binding footer and status bar all render. The status bar
correctly reflects the active model `qwen3:32b`.

### What remains unverified, and why

* **Keystrokes / submit / scroll / cancel / quit** — could not inject input reliably through
  either harness.
* **Streaming inside the TUI** — capture froze at exactly 2000 bytes for 90s. That is a pipe
  buffer boundary in my harness, not evidence about the app. I draw no conclusion.
* **Resize** — `ResizePseudoConsole` was unusable since the harness never captured output.
* **Clean exit / terminal restore** — my ANSI analysis found no `?1049h/l` alt-screen or
  `?25l/h` cursor sequences, **but I drove winpty with `-Xplain`, which strips escape
  sequences by design.** The analysis is therefore void. I explicitly do **not** claim the
  TUI fails to restore the terminal.

The mojibake visible in my box-drawing capture (`�"?`) is a `Get-Content` ANSI-decoding
artifact of mine, contradicted by the correctly-rendered `⠋` and the clean `* PONG` hex.
It is not a product defect.

---

## Stage 3 — the Phase 20 claims, live

### 3.1 Crash recovery — BROKEN (D2)

Killed a live turn with `Stop-Process -Force`, then attempted recovery. The journal files
were all present on disk (8 sessions, `.journal` / `.authority` / `.snapshot` /
`.writer.lock`). Recovery loads the session and then refuses to use it:

```
--continue        → Resumed session b303f4387d3b (3 messages, qwen3:32b model)
                    error: Session persistence authority unavailable: session has an
                    interrupted turn at journal cursor Some(322); resume, reconcile,
                    or cancel it before starting a new message
                    EXIT=1
--resume b303f4387d3b → identical, cursor Some(323), EXIT=1
```

**Control proving this is specific to interrupted turns, not to resume generally:**

```
--session-id abc123def456  "Reply with exactly ONE."  → EXIT=0, "* ONE"
--resume     abc123def456  "Reply with exactly TWO."  → EXIT=0, "* TWO"
                                Resumed session abc123def456 (2 messages, qwen3:32b model)
```

Clean sessions resume perfectly. A crash-interrupted one never can.

### 3.2 Process reaping — partial

The forced kill left no orphan from the killed tree (Stage 1.8). But the *stated* test —
spawn children, kill the parent, confirm no survivors — is **NOT VERIFIED**, because the
shell tool is refused in this session, so no tool child processes can be spawned at all.

### 3.3 Worktree / delegation lifecycle — NOT VERIFIED

Not reachable in this session: it depends on the shell/sandbox path that is gated off.

---

## Defects

| # | Sev | Title |
|---|---|---|
| D1 | HIGH | A refused tool call leaves a nonterminal tool execution and kills the session |
| D2 | HIGH | Crash-interrupted sessions are permanently unresumable; the fix named doesn't exist |
| D3 | HIGH | `backend = "plaintext"` disables every turn behind a misdirecting error |
| D4 | MED | `--list-sessions` prints nothing while 8 sessions exist on disk |
| D5 | MED | `--doctor` gives Linux-only remediation on Windows |
| D6 | MED | `--doctor` prints an MSYS Unix path on native Windows |
| D7 | LOW | TUI reports "Detected: AWS Bedrock" while running an entirely different provider |
| D8 | LOW | Three distinct root causes share one opaque error string |

### D1 — HIGH — refused tool call leaves a nonterminal tool execution, killing the session

A tool refusal does not terminate the tool-execution record. The journal then rejects the
state transition and the whole process exits 1.

```
> Read({"file_path":"C::\\wl-uat-work\\dir with spaces\\needle.txt"})
  X Refused to read C::\... : path must be absolute
error: Session persistence authority unavailable: invalid journal state transition:
       turn turn-c350f71e-89df-4ee4-8ee4-4ba915c8cc2a has nonterminal tool execution
       tool-execution-c9f6eda5-c082-4f35-8073-16cb01836b7f
EXIT=1
```

**This is sandbox-independent.** The refusal above is plain path validation — the model
emitted a malformed `C::\` path and Core correctly rejected it. No AppContainer involved.
Reproduced twice, via two different refusal sources (path validation, and the sandbox
refusal in §1.5).

**Control showing the bug is specific to the *refusal* path:** running the same Write without
`--force` produces an approval *denial*, which terminates cleanly:

```
  X Tool execution denied by user
  * The Write tool execution was denied by the system...
EXIT=0
```

Denial exits 0; refusal exits 1 with a corrupt journal transition.

**Why HIGH:** any model that emits a slightly malformed path — routine for smaller and local
models — destroys the whole session instead of letting the agent see the error and retry.
It is trivially reachable and session-fatal.

### D2 — HIGH — crash-interrupted sessions are permanently unresumable

Evidence in §3.1. The error instructs the user to "resume, reconcile, or cancel". Resume is
the thing that just failed, and **the other two do not exist**:

```
help mentions 'reconcile'  : False
help mentions 'cancel'     : False
help mentions 'interrupted': False
help mentions 'discard'    : False
help mentions 'abandon'    : False
```

None of the 20 subcommands offers a journal repair surface. The user has no way forward.
Each attempt advances the cursor (322 → 323) without resolving anything.

**This qualifies the Phase-20 crash-recovery claim.** Recovery *detects* and *loads* the
interrupted session correctly — the journal survived the kill and reports a precise cursor.
What is missing is the reconcile/cancel path that would let the session continue.

### D3 — HIGH — `backend = "plaintext"` disables every turn behind a misdirecting error

Clean A/B — same fresh profile, same deliberately-invalid key, one variable changed:

```
backend=auto      → exit=1  error: Provider error: API error 401 ... "invalid x-api-key"
                            request_id=req_011CdQ4R7Ga2FGnadNa5xUEz   (reached the provider)
backend=plaintext → exit=1  error: Session persistence authority unavailable: secure recovery
                            storage is unavailable; configure an OS keyring or encrypted
                            credentials vault                        (never reached the provider)
```

`open_confidential_store` rejects `Plaintext` outright
(`crates/wcore-config/src/credentials.rs:1441`). Refusing plaintext for confidential material
is correct security design; the defect is the consequence and the diagnosis:

* Total loss of function — no turn can run at all.
* The error never names the actual cause. It tells the user to configure a credentials
  backend when they already have one configured; it just happens to be the one value that
  is silently fatal.

**This is the live config on this box's real user profile** — `C:\Users\seand\AppData\Roaming\wayland-core\config.toml`
line 104 — so the installed CLI on this machine is unusable as configured. That is how this
defect was found: the very first live prompt of the pass failed on it.

**Bounding it:** `--init-config` does **not** emit a backend key, so a fresh install defaults
to `Auto` and is unaffected. This is not a broken-out-of-the-box defect.

### D4 — MEDIUM — `--list-sessions` prints nothing

`--list-sessions` returns `EXIT=0` and **zero output**, while 8 sessions with full journal
sets exist in the profile and `--continue` resolves one of them by name. Sessions are
invisible to the surface whose only job is listing them.

### D5 — MEDIUM — `--doctor` gives Linux-only remediation on Windows

```
[FAIL] chromium browser       NOT FOUND
       Install: apt install chromium-browser  (Debian/Ubuntu)
       Install: pacman -S chromium             (Arch)
       Install: nix-env -iA nixpkgs.chromium   (NixOS)
[WARN] BROWSERBASE_API_KEY    not set
       Hint: export BROWSERBASE_API_KEY=<key>
```

No winget/choco/scoop hint, and `export` is not a Windows shell builtin.

### D6 — MEDIUM — `--doctor` prints an MSYS Unix path on native Windows

```
[PASS] ollama    binary at /c/Users/seand/AppData/Local/Programs/Ollama/ollama
```

`/c/Users/...` is an MSYS translation leaking into user-facing output on native Windows;
the true path is `C:\Users\seand\AppData\Local\Programs\Ollama\ollama.exe`. This is the
documented Windows path-representation defect class (handoff §5.1) surfacing in the UI.

### D7 — LOW — TUI misreports the detected provider

The TUI onboarding line reads `Detected: AWS Bedrock` while the session was configured for
and actually used the OpenAI-compatible Ollama endpoint. The status bar in the same frame
correctly shows `qwen3:32b`, so the two disagree within one screen.

### D8 — LOW — one opaque error string for three distinct causes

`secure recovery storage is unavailable; configure an OS keyring or encrypted credentials
vault` was emitted for all of: the plaintext backend (D3), an unavailable OS keyring, and a
**wrong vault passphrase** (hit when my harness regenerated a random passphrase against an
existing vault). `RecoveryConfidentialError` deliberately collapses cause detail, which is
defensible for secrets, but it makes all three indistinguishable in the field.

---

## Explicitly NOT verified

| Item | Why | What would close it |
|---|---|---|
| Shell/Bash tool execution | Sandbox refuses; AppContainer unavailable in SSH session 0 | Run under the runner service or an interactive session |
| Any AppContainer / ACL / containment behavior | `ERROR_NO_SUCH_LOGON_SESSION`; probe fails at launch | Interactive or service context |
| Ctrl+C mid-turn cancellation | No console in session 0; ConPTY harness failed its control | Working `GenerateConsoleCtrlEvent` harness, or interactive session |
| TUI keystrokes, submit, scroll, cancel, quit | Could not inject input through ConPTY or winpty | Fixed ConPTY harness (`CharSet.Unicode`) or `pywinpty` |
| TUI streaming | Capture froze on a pipe buffer boundary | Same |
| TUI resize | Harness never captured output | Same |
| TUI clean exit / terminal restore | Drove winpty with `-Xplain`, which strips escapes | Re-run without `-Xplain` |
| Process reaping of tool children | Shell refused, so no children can be spawned | Working sandbox context |
| Worktree / delegation lifecycle | Depends on the gated shell path | Working sandbox context |
| Provider auth against a cloud provider | No valid key on the box (see below) | A valid provider key configured on the host |

**Credential blocker (reported, not worked around):** the box's configured Anthropic key is
16 characters — a truncated placeholder. The only full-length key present,
`ANTHROPIC_API_KEY_DIRECT`, returns `401 authentication_error: API key is invalid`. No secret
was printed, transmitted, or copied from the Mac. Live turns were driven instead against the
**local Ollama** instance already installed on the box, which needs no cloud credential.

---

## Bottom line

The Windows binary is real, correctly stamped at the sealed SHA, and does genuinely work as a
product: it launches, loads Windows-shaped config, completes real agent turns, streams
progressively without garbling, writes files through paths with spaces and 220 characters
deep, persists and resumes sessions, and renders a complete TUI.

Three HIGH defects sit on top of that. Two of them — D1 and D2 — are in exactly the
crash/journal machinery Phase 20A certified green, and both were invisible to the suite
because both require a *live* failure to surface: a model emitting a bad path, and a process
dying mid-turn. The third, D3, made the product unusable on this machine's real user profile
from the very first prompt.

That is the gap a passing suite could not see, and it is the answer to the question this pass
was created to ask.
