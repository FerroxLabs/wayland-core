---
lane: e2e-product-smoke
steps-total: 14
steps-passed: 12
steps-failed: 0
steps-not-reached: 2
new-finding: 2
credential-disclosure: hetzner-dsm, FluxRouter key injected on ssh stdin only, never in argv, never written to disk, swept 0/357 with a planted liveness control that scored 1/358
fence-exposure: 0 lines in crates/wcore-cli/src/lib.rs and main.rs; 0 .rs files changed vs 75babf32
status: PRODUCT WORKS END TO END
---

# E2E COLD-START PRODUCT SMOKE — the journey nobody had run

**Lane** `lane/e2e-product-smoke` · **base** `75babf32` · **host** `hetzner-dsm`
**Instrument** `wayland-core 0.12.25`, release, `--build-info` reports
`source 75babf329235484684ecee3a65973b0c197840c1` — byte-identical to my base, so the
stale-build class is closed by measurement rather than assumption.
**Model** `flux-standard` via FluxRouter. **Sandbox backend** `bubblewrap`.

---

## 1. The verdict

**The product works end to end.** A user who installs it cold on a headless Linux box,
points it at a provider and starts working gets: a real turn, five tools that do real
work on real files, a genuinely contained shell, a skill that takes effect, memory that
survives a session boundary, an MCP server that connects and answers, a session that
resumes after a restart, and a crash that leaves nothing running and nothing wedged.

Two things stand out as better than expected, and both were things this program had prior
reason to doubt:

- **Containment is real, not advertised.** A sandboxed shell command cannot read the
  product's own credential store or any file outside the workspace — not "was refused",
  but *does not exist in the child's mount namespace*. And the permitted arm succeeds in
  the same run, so this is not universal-denial green.
- **`owns_descendants_hard` holds under SIGKILL.** With a live sandboxed `sleep 300`
  grandchild, `kill -9` on wayland-core itself took the entire tree — bwrap, the shell,
  the sleep — to zero within 8 seconds.

Two findings, both MEDIUM, both about *what the user is told* rather than what the product
does. Neither blocks a day-one user.

**The most important caveat is about my own work, not the product:** my harness contained
four instrument defects, and the first three produced FAIL verdicts on steps whose product
behaviour was correct — including one that printed the exact opposite of what the sandbox
had done. Every one is documented in §5 with its repair and its self-test. **Had I reported
run 1, I would have filed four false findings, one of them a false security escape.**

---

## 2. Step-by-step results

| # | Step | Result | Evidence |
|---|---|---|---|
| 1a | Cold first run, nothing configured | **PASS** | rc=1 with an actionable error; no crash, no silent hang |
| 1b | Unhappy first-run paths (5 routes) | **PASS** | TOML errors give line/column/caret; typo'd sections give a per-key WARN |
| 2 | Provider configured, one real turn | **PASS** | `391` for 17×23, rc=0, token accounting printed |
| 3a-read | Read | **PASS** | recovered `NEEDLE_ALPHA_7731` from a file only readable by reading it |
| 3a-write | Write | **PASS** | 21-byte file on disk with exact content |
| 3a-edit | Edit | **PASS** | `status: COMPLETE`; old word gone, new word present |
| 3a-grep | Grep | **PASS** | located `buried.txt` among 18 files |
| 3a-glob | Glob | **PASS** | counted 17 `.log` files across two directory levels |
| 3b/3c | Bash through the sandbox, paired | **PASS** | permitted read+write succeed; outside-workspace read and write both blocked |
| 4 | Skill invoked and taking effect | **PASS** | canary token present with the skill, absent without it |
| 5 | Memory across a session boundary | **PASS** | recalled in a later process; isolated control did not know |
| 6 | MCP connect + `tools/call` | **PASS** | nonce round-tripped; server's own log records the call |
| 7 | Session resume after restart | **PASS** | reach asserted first; `list`/`show`/`resume` all rc=0 |
| 8a | Clean exit | **PASS** | rc=0, no surviving process |
| 8b | Crash exit (SIGKILL) | **PASS** | 5 descendants → 0 in 8s; product fully usable afterwards |
| — | TUI on a real terminal | **NOT REACHED** | see §6 |
| — | Windows / macOS cold start | **NOT REACHED** | see §6 |

**12 passed, 0 failed, 2 not reached.** Every FAIL that appeared during the work was
traced to my instrument and is accounted for in §5; none survived as a product failure.

---

## 3. The paired-permitted-case evidence

The brief is explicit that a refusal which passes because everything was denied is not a
pass. Every refusal below is reported next to a permitted case that **succeeded in the same
run, same binary, same workspace, same invocation shape**, and next to a control proving the
thing being denied is genuinely reachable from outside the sandbox.

`sandbox status --json` at the time of measurement:

```json
{"available":true,"backend":"bubblewrap","binds_cwd_authority":true,
 "binds_workspace_authority":true,"bypasses_containment":false,
 "enforces_read_deny":true,"owns_descendants_hard":true}
```

| Arm | Command | Inside sandbox | Outside control |
|---|---|---|---|
| **PERMITTED read** | `cat $WS/inside.txt` | **succeeds**, token visible | — |
| **PERMITTED write** | `echo … > $WS/sb-write.txt && cat it` | **succeeds**, read back | — |
| REFUSED — arbitrary file | `cat $OUT/outside.txt` | `No such file or directory` | readable, control=1 |
| REFUSED — **credential store** | `cat $WAYLAND_HOME/credentials.toml` | `No such file or directory` | readable, control=1 |
| REFUSED — `/etc/shadow` | `head -c 40 /etc/shadow` | `Refused: … credential-exfiltration denylist` | readable, control=1 |
| Allowed by design | `cat /etc/hostname` | succeeds | — |

Two points that make this evidence rather than theatre:

1. **`sandbox exec` is not a test hook.** It dispatches through
   `BashTool::execute_with_ctx` — the agent's own shell tool, the same function — so what
   it demonstrates is transitive to what the agent does.
2. **The refusal shape matters.** The two containment refusals are `No such file or
   directory`, i.e. the path is absent from the child's mount namespace. That is
   containment. The `/etc/shadow` refusal is a *denylist pattern match*, which is a
   different and weaker mechanism — and my first attempt at this step used `cat
   /etc/shadow` and got the denylist answer, which would have let me record a containment
   pass I had not earned. I re-ran with `/etc/hostname` (matches no denylist pattern) and
   with the credential store, which is the target a user actually cares about.

**The orphan pair, same discipline.** Before the kill, five descendants of wayland-core
were recorded **by PID from `/proc`**, and one of them was verifiably a real sandboxed
shell child:

```
797407 bwrap --die-with-parent --unshare-all --clearenv --new-session …
797408 wayland-core -m flux-standard --force --no-tui …
797409 bwrap --die-with-parent --unshare-all --clearenv --new-session …
797410 sh -c sleep 300
797411 sleep 300
```

`kill -9` to **wayland-core itself** (cmdline verified from `/proc/<pid>/cmdline` before
firing). 8 seconds later: **survivors = 0.** The permitted half of this pair is the
"a real child was alive at kill time" assertion — without it, "no orphans" would pass for
free on a process that never spawned anything, which is exactly what my first attempt
measured.

---

## 4. Findings

### FINDING A — the missing-credential error names 3 environment variables; the product consults 23 (MEDIUM)

**Measured, both directions, one run, fake keys only.** With `provider = "flux-router"`
configured, the error is:

> `No API key found. Provide via --api-key, config file, or environment variable (API_KEY, ANTHROPIC_API_KEY, or OPENAI_API_KEY).`

| arm | variable set | `No API key found` lines |
|---|---|---|
| control | none | 1 |
| named in the error | `ANTHROPIC_API_KEY` | **1 — inert** |
| named in the error | `OPENAI_API_KEY` | **1 — inert** |
| named in the error | `API_KEY` | 0 — works |
| **not named** | `FLUX_API_KEY` | 0 — works |

`resolve_api_key_from_env` reads **23** distinct `*_API_KEY` variables. Two of the three
the error recommends are inert for the configured provider and reproduce the identical
message; the variable that is actually correct is not mentioned. The generic `API_KEY` is
named and does work, so the text is *followable* — this is misleading, not dead, which is
why it is MEDIUM and not HIGH.

It is nonetheless the same defect class as the already-fixed headless-keyring HIGH: an
error whose named remedy does not fit the situation the user is in. Cheap fix: name the
active provider's variable, which the code already knows.

**Route to BACKLOG.** Non-blocking per the severity policy.

### FINDING B — the first-run warning promises a graceful fallback the session authority then refuses (MEDIUM)

On a headless host with no vault passphrase, the user sees, in this order:

```
warning: WAYLAND_HOME is set (isolated profile) but no vault passphrase was supplied;
         storing credentials as plaintext-0600 at …/credentials.toml.
…
error: Session persistence authority unavailable: secure recovery storage is unavailable:
       no OS keyring was usable and no encrypted credentials vault is unlocked.
```

The warning states the product is degrading gracefully on exactly the axis the error then
hard-fails on. The user reasonably concludes the first message means "you're fine", and is
stopped anyway. Note the product does substantial work before stopping — it registers
tools, spawns the cron scheduler, and **acquires the channel poll lease**
(`F24_CHANNEL_LEASE=owner`) — and only then refuses.

**The error text itself is correct and I verified it.** It names
`WAYLAND_VAULT_PASSPHRASE_FD` / `WAYLAND_VAULT_PASSPHRASE` and `[session] enabled = false`,
which is the prior lane's fix for `HEADLESS-KEYRING-FINDING.md`, and it is **present and
live at `75babf32`** — I set `WAYLAND_VAULT_PASSPHRASE` and the product started and
completed real turns. The finding is the *ordering and the contradiction*, not the remedy.

**Route to BACKLOG.**

### Positive observations worth recording

- **The product scrubs secrets out of MCP tool output.** Run 1's oracle returned
  `ORACLE_TOKEN=<32 hex>` and the user-visible output was `[REDACTED:SECRET_ASSIGNMENT`.
  That broke my probe and is *correct product behaviour* — an MCP server cannot launder a
  credential-shaped string through the agent into the transcript. I reshaped the token and
  the round trip then completed.
- **Config typos are caught loudly.** `[providrs.…]` and a mis-sectioned `[credentials]`
  each produce an explicit `ignoring unknown or mis-sectioned config key … check for a
  typo` WARN. This is the surface the prior headless-keyring lane found silently ignoring
  `[credentials] backend = …`; it now speaks up.
- **Session-ID validation is symmetric.** `--session-id e2e-resume-372360` is refused at
  **both** create and resume with `must be 6-40 hex characters`. My probe was wrong, not
  the product — and a product that accepted an ID at create and refused it at resume would
  have been a real defect, so this is worth stating positively.
- **BL-23B-H1 did not reproduce, on a reach-proven run.** Turn 1 dispatched a real tool
  event (`resume-marker.txt` on disk) before anything was believed. `session list` found
  the ID, `session show` rc=0 with `messages=7 turns=1 interrupted=0`, and the resumed
  process recovered the conversation. One observation is not a disproof and I am not
  claiming one; it is one more non-reproduction, this time with reach asserted rather than
  assumed.

### Not a finding, stated because absence is the easiest claim to fake

**5 lock files remain in `WAYLAND_HOME` after a crash** (`channel-poll.lock`,
`credentials.confidential-{backend,key}.lock`, `cron/schedule.lock`, one
`<id>.journal.writer.lock`). I am **not** reporting this as a defect, because the
operative question is whether they wedge the product, and the answer is measured: the very
next run after the SIGKILL returned rc=0 and the expected token. Stale-lock tolerance
works.

---

## 5. Instrument defects in my own harness — all four, with repairs

LANE-BRIEF §6b-ii is explicit that writing up an instrument defect without repairing it is
a defect you have agreed to keep, and that the one recorded recurrence on this program
happened for exactly that reason. So all four are repaired in-lane, each with a self-test.

**Defect 1 — `grep -c` double-counting.** `hits() { grep -c … || echo 0; }`. `grep -c`
*prints* `0` **and** exits 1 on no-match, so the `|| echo 0` appended a second line and the
function returned the two-line string `"0\n0"`. Every `[ "$x" = "0" ]` against a true
negative evaluated false.

*What it cost:* run 1 reported **FAIL** on `3a-edit` (the file on disk was correct),
on step 4 ("control leaked the token" — the control was clean), on `8a`/`8b` (process
counts were 0 and 0), and — worst — on the sandbox step it printed **"sandbox did NOT
refuse: shadow content leaked into child output"** when the command had in fact been
refused and nothing had leaked. *A gate that prints the opposite of what happened.*

*Repair + self-test (all three assertions pass):*
```
selftest 1/3 known-positive  PASS (got 1)
selftest 2/3 known-negative  PASS (got '0', compares equal to 0)
selftest 3/3 old-matcher-was-broken PASS (old returned '0/0/' which != 0)
```

**Defect 2 — `pgrep -f` self-matching.** The orphan probe searched for `sleep 240`, but
that string is **inside the prompt**, so it appears in the command line of the harness
subshell and of wayland-core itself. Proof: after the run, the only surviving match on the
entire host was the ssh command line of the query that went looking for them. Run 2's
`CHILD_ALIVE=1` was a false positive and `orphans=6` counted command lines. *Repair:* walk
`/proc` for real descendant PIDs before the kill; re-check those exact PIDs after. A PID
cannot self-match.

**Defect 3 — command substitution capturing narration.** A helper both printed its working
and `echo`ed its result, so `$(helper …)` captured the whole narration. The per-arm numbers
it printed were correct and unaffected; only the roll-up verdict was garbage — and it
graded **FAIL** on data that reads PASS. *Repair:* measurement and narration separated;
step A re-run and re-graded (PASS).

**Defect 4 — killing the wrong process.** The SIGKILL went to the harness's **wrapper
subshell**, not to wayland-core. Every "orphan" that produced is ordinary POSIX: killing a
shell does not kill its children, and reparenting to init is exactly what must happen.
Probe 3's section-B FAIL is therefore **withdrawn, not reported** — it did not test the
product at all. *Repair:* `exec` the binary so the backgrounded PID *is* wayland-core, then
**verify that from `/proc/<pid>/cmdline` before firing** (`VERIFIED: the SIGKILL target IS
the product, not a wrapper`).

Two self-test assertions came back **INCONCLUSIVE** and I am not counting them as passes:
the third assertion in probe 3 (`pgrep -f` on a marker that lived in a variable rather than
argv, so the original condition was not reproduced) and the third in probe 4 (bash
optimised the wrapper subshell into an exec, so there was no separate wrapper to
demonstrate). In both cases the defect is nonetheless established by direct evidence from
the failing run itself — for defect 4, probe 3's own output shows victim `688195` with
wayland-core `688200` listed as its *descendant*, which is only possible if they are
different processes.

**The general lesson this lane paid for four times over:** on a smoke test, the harness is
as likely to be broken as the product, and a broken harness fails *toward* alarming
verdicts. Every FAIL here needed adjudication before it was worth anything.

---

## 6. What I did NOT do, and why

- **The TUI on a real terminal — NOT REACHED.** Every run used `--no-tui` because the
  harness drives the binary over ssh with `stdin < /dev/null`. The ratatui surface, the
  onboarding flow reached by `wayland-core setup`, and the `· FORCE` badge are therefore
  **unexercised**. This matters: the TUI is the default surface for an interactive user on
  a TTY, so the *literal* day-one path is the one leg I did not drive. It needs a pty
  harness; I did not build one.
- **Windows and macOS cold start — NOT REACHED.** Linux/bubblewrap only. The containment
  and orphan results above are Linux results and must not be read as cross-platform.
- **`wayland-core setup` / onboarding — not exercised** beyond confirming `init`
  scaffolds `.wayland/config.toml` + `WAYLAND.md`.
- **Fixed neither finding.** Both are MEDIUM; the severity policy routes MEDIUM to BACKLOG
  and explicitly warns against inventing a stricter rule. Both are small, contained text
  changes that a follow-up can take.
- **No Rust changed, no test weakened, no `#[ignore]` added, no gate re-defined.**
- **Reserved actions not taken:** no merge, no PR, no tag, no release, no issue closed, no
  `wcore-contract generate`, no `.github/workflows/*` touched.

---

## 7. Credential handling

| | |
|---|---|
| Machine | `hetzner-dsm` |
| Method | loaded on the Mac via `set -a; . ~/.wayland-secrets/flux.env; set +a`, then piped to `ssh` **on stdin**; the remote shell did `IFS= read -r K; export FLUX_API_KEY="$K"; unset K` and `exec`'d the harness with stdin redirected to `/dev/null` |
| Never | in `argv` on either host, in a log, in a capture, in a commit, in this report |
| Redaction | every capture passed through `sed s/<key>/<REDACTED_FLUX_KEY>/g` plus an `sk-[A-Za-z0-9_-]{20,}` catch-all, at write time |

**Sweep, with the mandated planted liveness control.** Tool: `scripts/f24-secret-sweep.sh`,
whose own 5-assertion self-test passes on both hosts (`selftest all_pass=true`) — including
its third assertion, that the collapsed-path invocation which produced this program's
`"0 hits, clean" from a dead grep` **misses** a planted secret.

| host | paths | readable text files traversed | hits |
|---|---|---|---|
| Mac | evidence dir + notes **+ planted copy** | 9 | **1** ← control alive |
| Mac | evidence dir + notes | 8 | **0** |
| hetzner | 6 output trees, 5 logs, 7 scripts **+ planted copy** | 358 | **1** ← control alive |
| hetzner | same, plant removed | 357 | **0** |
| Mac | logs pulled back into the repo | 5 | **0** |

`CLEAN (control alive, 0 value hits)`. The script refuses to report clean unless the
aliveness check over *the caller's actual paths* found readable text first, so a wrong
path, an eaten glob or an errored tool cannot produce a clean result here.

---

## 8. Fence exposure vs `75babf32`

Merge-base captured once and quoted, per §6:

```
BASE=75babf329235484684ecee3a65973b0c197840c1
git diff "$BASE" -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs  ->  0 lines
```

Known-positive control for the same instrument, same invocation shape: the same command
against a file I *did* change reports **144 lines**. So the zero is a measurement, not a
dead command.

**0 `.rs` files changed.** Eight files added, all under `.planning/`:

```
A .planning/E2E-PRODUCT-SMOKE-NOTES.md
A .planning/evidence/e2e-product-smoke/{journey,journey2,probe3,probe4,s1-coldstart}.sh
A .planning/evidence/e2e-product-smoke/mcp_oracle_server{,2}.py
```

Nothing for the orchestrator to serialize: no protocol seam, no contract request, no
shared-file edit.

---

## 9. Reproduction

```bash
ssh hetzner-dsm
git -C /root/wayland worktree add -b hz/e2e-product-smoke /root/wayland-e2e 75babf32
export PATH=/root/.cargo/bin:$PATH
cargo build --release -p wcore-cli          # 5m45s, BUILDRC=0

# credential on stdin only
printf '%s\n' "$FLUX_API_KEY" | ssh hetzner-dsm '
  IFS= read -r K; export FLUX_API_KEY="$K"; unset K
  export BIN=/root/wayland-e2e/target/release/wayland-core MODEL=flux-standard
  bash /root/wayland-e2e/journey2.sh /root/wayland-e2e/out-j2'
```

`s1-coldstart.sh` needs no credential. `journey2.sh` refuses to run at all if its
three-assertion instrument self-test fails. `probe4.sh` closes steps 3b/3c and 8b with the
repaired instruments. Raw redacted logs are committed under
`.planning/evidence/e2e-product-smoke/logs/`.
