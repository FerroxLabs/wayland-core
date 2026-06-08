# Contributing to Wayland-Core

## Security-Critical Dependency Pinning

A curated set of trust-boundary crates are **exact-pinned** (`=X.Y.Z`) in the
workspace `[workspace.dependencies]` table rather than the usual caret
(`^X.Y`) range. The lockfile pins exact versions for every build, but a
caret range still lets `cargo update` pull an unreviewed patch or minor
release silently. Exact-pinning closes that gap for code that sits on a
trust boundary (signature verification, vault-at-rest crypto, auth
signing, OS keychain, TTY-secret input).

### Currently exact-pinned (1.D.2)

| Crate              | Pinned version | Reason                              |
|--------------------|---------------:|-------------------------------------|
| `ed25519-dalek`    | `2.2.0`        | plugin signature verification       |
| `argon2`           | `0.5.3`        | vault KDF                            |
| `chacha20poly1305` | `0.10.1`       | vault AEAD                           |
| `zeroize`          | `1.8.2`        | secret-buffer wipe                   |
| `jsonwebtoken`     | `10.3.0`       | Vertex auth JWT                      |
| `aws-sigv4`        | `1.4.3`        | Bedrock request signing              |
| `keyring`          | `3.6.3`        | OS keychain backend                  |
| `rpassword`        | `7.5.2`        | TTY-secret input                     |
| `rustls-webpki`    | `0.103.13`     | already pinned (RUSTSEC-2026-0098+) |

### Bumping a pinned dep

Bumping a pin requires an explicit review pass:

1. **Open the upstream release notes and changelog** — read every entry
   between the current pin and the candidate. Flag any security-relevant
   change (new RUSTSEC advisory, key-format migration, default-algorithm
   change, transitive crypto-crate bump).
2. **Run `cargo audit` and `cargo deny check advisories`** against the
   bumped lockfile. Note: `cargo audit` is informational; treat any
   `error: vulnerability found` as blocking.
3. **Update both `Cargo.toml` and `Cargo.lock`** in the same commit.
   Commit message must reference the upstream changelog URL.
4. **Cross-link** the PR description to any RUSTSEC advisory or security
   release notes you read while reviewing.

### When to add a new exact-pin

A new dep belongs on this list if **any** of the following are true:

- The crate implements a cryptographic primitive (signing, AEAD, KDF, MAC,
  hash-with-secret).
- The crate sits between a secret and the network (auth signing,
  credential storage, TLS configuration).
- The crate handles a secret at rest (vault, keychain, env-var reader for
  secrets).
- A compromised release would let an attacker bypass a trust boundary
  even with the rest of the supply chain intact.

Pure functional / data-shape / parser crates do **not** belong here even
if they're widely used (e.g., `serde`, `clap`, `toml`).

## Other conventions

- Workspace-wide `cargo fmt --all -- --check` must pass before commit
  (methodology #22).
- Workspace `cargo build --release -p wcore-cli` must succeed before
  running the full test gate (methodology #23).
- Every new public symbol must have at least one production caller in
  the same PR (methodology #27 — no orphan APIs).
- Never push with `--force` to a shared branch and never amend a pushed
  commit; create a new commit instead.
