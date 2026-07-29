# 24-EMAIL-MSTEAMS — running NOTES (append-only, committed per §6b-i)

Lane: `lane/24-email-msteams`, worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-email-msteams`,
base `e77b44b0` (verified equal to `gh/plan/f20-unified-audit-repair` at fetch time).

Tasks: (1) close email's reply half by wiring a TLS root through; (2) cost msteams
from source rather than inheriting the "discord-shaped, ~2 sessions" estimate.

---

## T+0:15 — orientation committed before investigating

### Established so far (read from source, not inherited)

**E1. `LettreSender::new` passes no TLS parameter.** `crates/wcore-channel-email/src/smtp.rs:71-84`:

```rust
let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
    .map_err(|e| EmailError::Smtp(format!("build relay {host}: {e}")))?
    .port(port)
    .credentials(creds)
    .build();
```

There is no `.tls(...)` call. This matches the brief's framing: the knob exists in
lettre and nothing in this crate turns it. Confirmed by an unproxied grep for
`Tls|tls|starttls|builder_dangerous` over `smtp.rs` — the only hits are the
`starttls_relay` line, the doc comment, and `SendError::Transient`'s "TLS" mention.
No `TlsParameters` construction anywhere in the crate.

### Still to establish (do not report any of these as fact yet)

- **U1.** Verify the brief's central claim *in lettre's own source*, at the pinned
  version, not from the brief: that `add_root_certificate` is additive on top of the
  compiled-in roots and does NOT disable verification. The brief cites `tls.rs:248`
  and `tls.rs:518-526`. Until read at the resolved version this is a second-hand claim.
- **U2.** Which TLS backend feature is enabled for lettre in this workspace
  (native-tls vs rustls). This decides both the API and the platform coverage claim.
- **U3.** Whether an SMTP-side live proof is reachable on hetzner (the inbound leg
  reportedly is). Need counts, not a "no error" pass.
- **U4.** msteams shape: is it really "discord-shaped"? Read `lib.rs`/`inbound.rs`
  for the transport (Discord's cost came from a WebSocket gateway). Two prior
  costings each had one detail wrong, so this must come from source.
- **U5.** `README.md:348` claims MS Teams is send-only MVP whose inbound "is parsed
  but not yet exposed over the host." Check against current code — if still true, the
  inbound clause may not be buildable without product work, and that is the finding.

### Traps I am explicitly holding (from LANE-BRIEF §3b-i, §3.2)

- Do NOT use `dangerous_accept_invalid_certs` — a prior lane refused it as a
  production security regression shipped to pass a test. That judgement stands.
- Do NOT conflate this with the macOS `SSL_CERT_FILE`/Security.framework IMAP
  limitation. Different mechanism. My summary must state platform coverage explicitly.
- Any absence claim needs a known-positive in the same invocation + unproxied tool +
  the query stated.
- Prove arrivals with positive counts; assert executed test counts (`N passed`),
  never bare exit status.

### Housekeeping

- Nothing built yet. Mac is fmt-only per §0; compilation goes to hetzner.
- No files modified outside `.planning/phases/24-.../` yet.
- Will NOT touch `scripts/f24-inbound.mjs`, `crates/wcore-channel-matrix/`,
  `.github/workflows/ci.yml`, `crates/wcore-cli/src/{lib,main}.rs`, `.planning/BACKLOG.md`.
