# FINDING — the headless keyring remedy is advertised-but-dead

**Lane** `lane/headless-keyring` · **base** `plan/f20-unified-audit-repair` @ `b79f141e`
**Severity** **HIGH** · **Status** measured, fixed, and re-proven live in this lane
**Instrument** `wayland-core 0.12.25`, release, built from this branch on `hetzner-dsm`, `BUILDRC=0`

---

## 1. The question

Phase 30-02, running the real product against two competitor harnesses, found that
`wayland-core` refuses to start headless, and that **neither peer needs an equivalent**. The
error names its own remedy. The only question this lane asked:

> **Does a headless operator, following that remedy exactly as written, get a working
> `wayland-core`?**

Answer: **no — by any reading of it, including the fully corrected one.**

---

## 2. Verdict per route

Condition under test: a headless Linux host with **no OS keyring**. Provider contact is
measured from a loopback mock's **own** request log, never from the product's stdout, so
"started" is never confused with "completed a turn". Every route ran a real one-shot turn.

| # | What the operator does | rc | provider contact | turn completed | verdict |
|---|---|---|---|---|---|
| R0 | nothing (control) | 1 | no | no | **error reproduced — condition established** |
| R1a | `[credentials] backend = "encrypted-file"` — *the text, literally* | 1 | no | no | **DEAD** — key ignored, identical error re-emitted |
| R1c | `[storage.credentials] backend = "encrypted-file"` | 1 | no | no | **DEAD** — hard parse error, config will not load |
| R1e | `[storage.credentials] backend = "encrypted_file"` | 1 | no | no | **DEAD** — hard parse error (struct variant) |
| R1h | `[storage.credentials.backend.encrypted_file]` + 2 invented paths + env var | 0 | yes | **yes** | works, but is 3 corrections from the text and appears in no doc |
| R1g | **`WAYLAND_VAULT_PASSPHRASE` only, no config change at all** | 0 | yes | **yes** | **the actual remedy — named nowhere in the product** |
| R2 | `[session] enabled = false` | 0 | yes | **yes** | works; key not named in the error text either |
| R5 | plain default install, `WAYLAND_HOME` unset | 1 | no | no | **error reproduced — this is not isolated-profile-only** |
| R6 | R5 + `WAYLAND_VAULT_PASSPHRASE` | 0 | yes | **yes** | works |

Full route table incl. R1b/R1d/R1f/R1i in
`.planning/evidence/headless-keyring/HEADLESS-KEYRING-NOTES.md`; raw stdout/stderr for all
of them in `.planning/evidence/headless-keyring/transcripts/`.

---

## 3. What was actually wrong — three independent defects in one sentence

The message said:

> …Configure an OS keyring, or set `credentials.backend` to `"encrypted-file"` and supply its
> unlock passphrase

**(a) `credentials` is not a section.** The schema is `[storage.credentials]`
(`crates/wcore-config/src/credentials.rs:78`). An operator who writes what the text says gets:

```
WARN ignoring unknown or mis-sectioned config key `credentials` in .../config.toml
     — it has no effect; check for a typo or wrong [section] key=credentials
error: Session persistence authority unavailable: … set credentials.backend to
       "encrypted-file" and supply its unlock passphrase
```

The product tells you to set a key, then tells you that key has no effect, then tells you to
set it again. **That is the closed loop, measured verbatim** — the same shape as `27-C2(a)`.

**(b) `"encrypted-file"` is not a value the parser accepts.** At the *correct* section it is a
hard failure, and the config stops loading entirely — so following the advice is strictly
worse than ignoring it:

```
14 | backend = "encrypted-file"
   |           ^^^^^^^^^^^^^^^^
unknown variant `encrypted-file`, expected one of `auto`, `plaintext`, `keyring`, `encrypted_file`
```

**(c) It can never be a bare string in any spelling.** Correcting to the underscore the parser
itself just suggested still fails, because `CredentialsBackend::EncryptedFile { cipher_path,
key_params_path }` (`credentials.rs:56-61`) is a **struct variant**:

```
14 | backend = "encrypted_file"
   |           ^^^^^^^^^^^^^^^^
invalid type: unit variant, expected struct variant
```

**(d) And the passphrase half names no mechanism at all.** "supply its unlock passphrase"
never says how. Measured coverage of `WAYLAND_VAULT_PASSPHRASE`: **0** files under `docs/`,
**0** bytes of `--help` (13,921 bytes searched), **0** error messages. `--doctor` probes
`wlrctl`, `grim`, `chromium`, `ollama`, `WAYLAND_DISPLAY` — not the credential authority that
is actually blocking startup. The single string in the entire product that names the env var
is the plaintext-fallback warning, and that warning is `WAYLAND_HOME`-gated, so it does **not**
print on a default install (absent from R5, present in R0). The one hint that exists is
suppressed in exactly the case that needs it.

---

## 4. Severity: HIGH

Cross-audit panel (§4), all three voted independently:

| voter | position |
|---|---|
| codex `gpt-5.6-sol` | `PANEL_POSITION=HIGH` — "every remediation the error explicitly recommends is invalid or incomplete… not CRITICAL because there is a reliable workaround" |
| gemini `3.1-pro-preview` | `PANEL_POSITION=HIGH` — "actively gaslights users into a dead-end configuration loop with factually incorrect schema advice" |
| kimi K3 | `PANEL_POSITION=HIGH` — "not CRITICAL… the process fails closed and safe" |

**Internal adversarial pass, arguing for LOWER.** This program has already graded this symptom
**LOW twice** — BACKLOG `F21-02-08` ("Not a defect — an environment requirement") and
`24-C3-SUMMARY:398`, which scoped it explicitly to *"an isolated profile"*. If it only bit
isolated profiles, MEDIUM would be the ceiling and I would be inflating a known-LOW.

**Tested, and the objection fails.** R5 — plain default install, `WAYLAND_HOME` unset, config
at the real default `$HOME/.config/wayland-core/config.toml` — reproduces the identical error.
`session.enabled` defaults to `true` (`config.rs:991-1008`), and the preflight sits on the
journaled path (`engine.rs:6105`), so **the default configuration of a default install is the
failing one** on any host without a Secret Service: containers, CI runners, minimal cloud VMs.
The prior LOW gradings were scoped too narrowly. The adversarial pass also turned up an
*aggravating* fact (§3d), not a mitigating one.

Not CRITICAL: no data loss, no corruption, no security boundary crossed — it fails closed, and
a working escape does exist once you know it. HIGH is where the evidence lands.

**Also worth stating plainly:** `hetzner-dsm` runs `gnome-keyring-daemon --components=secrets`
with `org.freedesktop.secrets` live on the session bus. That is why other lanes ran headless
all night without seeing this — **the gate is conditional and they were never in the
condition**, not because it is narrow. A headless *build box that happens to have a desktop
keyring* is not a stand-in for a headless *deployment*.

---

## 5. The fix — landed and re-proven in this lane

`crates/wcore-agent/src/recovery_confidential.rs`, commit `eabb6ec0`. Both refusal strings now
name what was **measured to work**, and two gates re-parse the advertised values through the
real `CredentialsStorageConfig` / `SessionConfig`, so no future edit can advertise an
unrepresentable one again.

**RED→GREEN, with executed counts read back (never exit status):**

- gates vs. the pre-fix strings, new tests only swapped in: `test result: FAILED. 9 passed; 2 failed`
- gates vs. the fix: `test result: ok. 11 passed; 0 failed`
- D3 regression suite unchanged and green: `3 passed; 0 failed`
- `cargo fmt --all -- --check` clean

**Live, on the rebuilt binary, plain default install, no keyring** — the message an operator
now sees, and both remedies in it followed literally:

```
error: Session persistence authority unavailable: secure recovery storage is unavailable:
no OS keyring was usable and no encrypted credentials vault is unlocked. On a headless host
set WAYLAND_VAULT_PASSPHRASE_FD (a passphrase file descriptor — preferred) or
WAYLAND_VAULT_PASSPHRASE to unlock the encrypted vault, or turn durable sessions off with
[session] enabled = false

P0 no remedy                        rc=1 contact=no  stdout=[]
P1 WAYLAND_VAULT_PASSPHRASE=…       rc=0 contact=yes stdout=[* HEADLESS_TURN_OK]
P2 [session] enabled = false        rc=0 contact=yes stdout=[* HEADLESS_TURN_OK]
```

Before the fix, no reading of the text reached a completed turn. After it, both do.

### What this fix does NOT do — deliberately

- **It does not document anything.** `WAYLAND_VAULT_PASSPHRASE` still appears in 0 files under
  `docs/` and 0 bytes of `--help`. The error text is no longer a dead end, but the docs gap is
  untouched and I did not open it — it is not provable by a gate in this lane.
- **It does not change the security posture.** Refusing plaintext for confidential material is
  the design and is untouched. Only the wording of the refusals changed.
- **It does not make the default work out of the box.** A headless host still needs one line
  that two competing CLIs do not. Whether that asymmetry should exist at all is a product
  decision, not a lane decision.

### Recommended follow-ups (NOT done here)

1. Document the vault transports in `docs/getting-started.md` — one paragraph.
2. Make `--doctor` probe the credential authority, which is the check that would have caught
   this before a user did.
3. Consider whether `Auto` should self-unlock an ephemeral vault on a keyring-less host, so
   the default install simply works. That is an architecture call, deliberately left open.

---

## 6. Reproduce

```bash
export BIN=<release wayland-core>; export OUT=/tmp/hlkr-out
bash .planning/evidence/headless-keyring/run-routes.sh
```

R0 is the harness's own gate: if it does **not** reproduce the keyring error, the condition was
not established and every other row is void. It caught one real harness bug during this lane —
an early revision wrote `wcore.toml` instead of `config.toml`, and R0 failing to reproduce is
what exposed it.
