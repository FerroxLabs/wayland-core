# NOTES — headless keyring remedy lane (live, append-only)

Lane branch `lane/headless-keyring`, base `plan/f20-unified-audit-repair` @ b79f141e.
Question: **does a headless operator, following the remedy text exactly as written, get a
working `wayland-core`?**

## Established so far (Mac-side source/doc reading only — nothing live yet)

### E1. The two remedy strings, verbatim

`crates/wcore-agent/src/recovery_confidential.rs:81-86`:

```
"secure recovery storage is unavailable: no OS keyring was usable and no encrypted
 credentials vault is unlocked. Configure an OS keyring, or set credentials.backend to
 \"encrypted-file\" and supply its unlock passphrase"
```

`recovery_confidential.rs:75-80` (sibling, plaintext backend):

```
"credentials.backend is set to \"plaintext\", which cannot hold the confidential key that
 durable session recovery requires. Set credentials.backend to \"keyring\" or
 \"encrypted-file\", or disable session persistence"
```

Wrapped by `crates/wcore-agent/src/engine.rs:20948`
`#[error("Session persistence authority unavailable: {0}")]`.

### E2. The unlock mechanism exists in code — two transports

`crates/wcore-config/src/credentials.rs:1104-1110` `vault_unlock_material_present()`:
- `WAYLAND_VAULT_PASSPHRASE_FD` (unix only — a passphrase file descriptor, preferred)
- `WAYLAND_VAULT_PASSPHRASE` (env var; comment calls it "legacy")
- interactive `rpassword` prompt is deliberately NOT counted as present.

### E3. **Documentation coverage of the remedy is ZERO.** (measured, not assumed)

Counted files under `docs/` + `README.md` containing each literal:

| literal | files |
|---|---|
| `WAYLAND_VAULT_PASSPHRASE` | **0** |
| `credentials.backend` | **0** |
| `encrypted-file` | **0** |
| `keyring` | 3 |

So the error text names a config key documented nowhere, and instructs the operator to
"supply its unlock passphrase" without naming any mechanism by which to supply it. This is
the brief's stated trap in its purest form: to follow the remedy I had to read
`credentials.rs`. **That gap is itself a finding**, independent of whether the remedy works.

### E4. Prior art in this program — same symptom, graded LOW twice

- `.planning/BACKLOG.md` F21-02-08 (LOW): "a hermetic live run needs an ephemeral vault…
  Not a defect — an environment requirement any future live harness must honour." Points at
  `crates/wcore-cli/tests/support/vault.rs` which provides BOTH transports.
- `.planning/phases/24-.../24-C3-SUMMARY.md:398` (LOW → BACKLOG): "An isolated profile with
  no vault passphrase stores credentials plaintext-0600 and then refuses **every** turn."
- `30-02-TRIAL-RESULTS.md` §4: reached a provider only after being given
  `[credentials] backend = "encrypted-file"` + a vault passphrase; neither peer needed one.

**Note the tension**: 24-C3 says it refuses *every turn* after falling back to plaintext-0600,
which is the `PlaintextBackendRejected` arm, a DIFFERENT arm from the one 30-02 hit. Two
distinct arms, same operator-visible dead end. Must keep them apart in the report.

### E5. **R4 ANSWERED — the gate is conditional, and hetzner has a live keyring.**

Measured on `hetzner-dsm` (`Ubuntu-2404-noble-amd64-base`):

```
DBUS_SESSION_BUS_ADDRESS=[unix:path=/run/user/0/bus]
/usr/bin/gnome-keyring-daemon
2531915 /usr/bin/gnome-keyring-daemon --start --foreground --components=secrets
2693804 /usr/bin/gnome-keyring-daemon --start --foreground --components=secrets
2694287 /usr/bin/gnome-keyring-daemon --foreground --components=pkcs11,secrets \
        --control-directory=/run/user/0/keyring
$ dbus-send --session ... ListNames | grep -i secret
      string "org.freedesktop.secrets"
```

So the other lanes ran `wayland-core` headless successfully **because the box has a working
Secret Service on the session bus** — not because the gate is narrow. `hetzner-dsm` is a
headless box that nevertheless has a desktop keyring daemon running, so it is NOT a valid
stand-in for a real keyring-free host. Answer to R4: **conditional gate, and the other lanes
were simply never in the condition.** A second, partial route also exists — `22-01-JOURNAL-COMPAT.md:26`
documents `export WAYLAND_VAULT_PASSPHRASE=<throwaway>`, i.e. at least one lane *did*
configure around it, with a comment saying an isolated profile refuses durable sessions
without it.

Consequence for method: the condition under test must be **created** (no session bus / no
secrets service), which is what a container, a CI runner or a minimal cloud VM actually is.

### E6. Structural: the gate is reached ONLY on the journaled path

`crates/wcore-agent/src/engine.rs:6078` — `if self.session_journal.is_none() { … return }`
runs the turn WITHOUT ever calling `preflight`. The preflight at `engine.rs:6105-6108` is
therefore only reachable when a session journal is bound. `config.session.enabled`
(`wcore-config/src/config.rs:991-1008`) defaults to **`true`** (`#[serde(default = "default_true")]`),
so the journaled path is the DEFAULT path.

This also identifies what remedy route 2 ("disable session persistence") must mean in
practice: `[session] enabled = false`. Note that key is not named anywhere in the error text.

## LIVE RESULTS — binary built at this lane's own SHA

`/root/wayland-hlkr/target/release/wayland-core`, `wayland-core 0.12.25`, built from
`09686599` (this branch), `BUILDRC=0`. Condition created by stripping the session bus
(`env -u DBUS_SESSION_BUS_ADDRESS -u XDG_RUNTIME_DIR -u DISPLAY`) — i.e. a container / CI
runner / minimal VM. A loopback OpenAI-compatible mock stands in for the provider so that
"started" can be told apart from "completed a turn"; contact is measured from the **mock's
own log**, never the product's stdout.

**Harness self-check fired once, as designed.** The first revision wrote `wcore.toml`; R0
then failed with "No API key found" rather than the keyring error — the control did not
reproduce, which is what caught it. Corrected to `config.toml` (confirmed against
`wayland-core --config-path`). Kept R0 as the harness's own gate.

| route | config written | passphrase | rc | provider contact | outcome |
|---|---|---|---|---|---|
| R0 control | none | no | 1 | no | **keyring error reproduced** — condition established |
| R1a | `[credentials] backend="encrypted-file"` (literal text) | no | 1 | no | key **ignored**, identical error re-emitted |
| R1b | same literal | yes | 0 | yes | turn completed — *but the config clause was ignored; the passphrase did it* |
| R1c | `[storage.credentials] backend="encrypted-file"` | no | 1 | no | **hard TOML parse error, config unloadable** |
| R1d | same | yes | 1 | no | **hard TOML parse error** |
| R1e | `[storage.credentials] backend="encrypted_file"` | yes | 1 | no | **hard TOML parse error** (unit vs struct variant) |
| R1f | same | no | 1 | no | **hard TOML parse error** |
| R1g | **none at all** | yes | 0 | yes | **turn completed** |
| R1h | `[storage.credentials.backend.encrypted_file]` + 2 paths | yes | 0 | yes | turn completed |
| R1i | same | no | 1 | no | error blames a "wrong unlock passphrase" never supplied |
| R2 | `[session] enabled = false` | no | 0 | yes | **turn completed** |

Verbatim, R0 (and R1a, identically):

```
error: Session persistence authority unavailable: secure recovery storage is unavailable:
no OS keyring was usable and no encrypted credentials vault is unlocked. Configure an OS
keyring, or set credentials.backend to "encrypted-file" and supply its unlock passphrase
```

R1a, having been told to set `credentials.backend`, and having set exactly that:

```
WARN ignoring unknown or mis-sectioned config key `credentials` in .../config.toml
     — it has no effect; check for a typo or wrong [section] key=credentials
```
…followed by the **identical** error again. That is the closed loop, measured.

R1c/R1d — the same value at the section the schema actually defines:

```
Error: failed to parse .../config.toml: TOML parse error at line 14, column 11
14 | backend = "encrypted-file"
   |           ^^^^^^^^^^^^^^^^
unknown variant `encrypted-file`, expected one of `auto`, `plaintext`, `keyring`, `encrypted_file`
```

R1e — spelling corrected to the underscore the parser just asked for:

```
14 | backend = "encrypted_file"
   |           ^^^^^^^^^^^^^^^^
invalid type: unit variant, expected struct variant
```

because `CredentialsBackend::EncryptedFile { cipher_path, key_params_path }`
(`credentials.rs:56-61`) is a **struct** variant: it can never be a bare string in any
spelling. R1h is the only config form that loads, and it requires inventing two paths.

R1i — right backend, no passphrase:

```
error: Session persistence authority unavailable: secure recovery storage could not be
read: the configured store rejected this profile's recovery key. An encrypted vault opened
with the wrong unlock passphrase reads this way — re-check the passphrase for this profile
```

i.e. it tells the operator to re-check a passphrase they never set, and still never names
`WAYLAND_VAULT_PASSPHRASE`.

### R3 — `--help` (13,921 bytes) offers a headless operator nothing

`keyring` 0 hits, `vault` 0, `passphrase` 0, `WAYLAND_VAULT` 0, `credential` 1.
`--doctor` probes `wlrctl`/`grim`/`chromium`/`ollama` and `WAYLAND_DISPLAY`/`DISPLAY` —
it does not probe the credential/keyring authority that actually blocks startup.

## Verdict

**Route 1 (`credentials.backend = "encrypted-file"`) is ADVERTISED-BUT-DEAD.** Every literal
reading of it either has no effect (and re-emits the identical error) or hard-fails config
parsing. It is wrong in three independent ways at once — section, spelling, and shape — and
no operator can recover from it using only product-supplied text, because `WAYLAND_VAULT_PASSPHRASE`
appears in **no** doc, **no** `--help`, and **no** error message. The one string that does
name it is the *plaintext-fallback warning*, which is only printed when `WAYLAND_HOME` is set.

**Route 2 (`[session] enabled = false`) WORKS** — but that key is not named in the error text
either, and it works by turning off durable sessions, i.e. by giving up a feature.

**The actual one-line fix is `WAYLAND_VAULT_PASSPHRASE=<anything>` (R1g), which the product
never tells anyone.** Adding the advertised config line on top of it *breaks* it (R1d).
