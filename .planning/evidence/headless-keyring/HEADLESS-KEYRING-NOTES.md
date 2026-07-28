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

## Still to establish (live, hetzner, no OS keyring installed)

- [ ] R0: reproduce the bare failure headless (control — the gate must be able to fail)
- [ ] R1: remedy route 1 — `credentials.backend = "encrypted-file"` + passphrase. Does it
      START and COMPLETE A REAL TURN, or land in a second error?
- [ ] R2: remedy route 2 — "disable session persistence". **Does a flag/config for this even
      exist?** If not, that string is advertised-but-dead on its face.
- [ ] R3: anything `--help` points a headless operator toward.
- [ ] R4: why did other lanes run headless on hetzner fine tonight? Conditional gate, or did
      they configure around it? (cheap check against their evidence)

## Verdict so far

NOT YET REACHED. E3 is solid and already reportable. Severity depends entirely on R1/R2.
