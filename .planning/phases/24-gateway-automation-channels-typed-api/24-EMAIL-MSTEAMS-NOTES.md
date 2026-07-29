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

---

## T+0:30 — U1 and U2 resolved from source. The platform answer is sharper than expected.

### E2. The brief's lettre claim is CORRECT, verified at the pinned version

lettre resolves to **0.11.22** (`Cargo.lock`). Source pulled from
`static.crates.io/crates/lettre/lettre-0.11.22.crate` and read directly
(`src/transport/smtp/client/tls.rs`, 862 lines).

`add_root_certificate` (tls.rs:248) is a pure push:

```rust
pub fn add_root_certificate(mut self, cert: Certificate) -> Self {
    self.root_certs.push(cert);
    self
}
```

At build time (tls.rs:518-526) those roots are added on top of whatever base store
`CertificateStore` selected:

```rust
for cert in self.root_certs {
    for rustls_cert in cert.rustls {
        root_cert_store.add(rustls_cert).map_err(error::tls)?;
    }
}
```

The verification-disabling branch immediately below is gated **solely** on
`self.accept_invalid_certs || (extra_roots.is_none() && self.accept_invalid_hostnames)`.
Adding a root does not touch either flag. **So the reply half is reachable with full
chain + hostname verification intact.** The determination in the brief stands, and it
now rests on read source rather than on the brief.

### E3. The two email legs use DIFFERENT TLS backends — this is the platform answer

This is the concrete reason the brief's "do not conflate with the macOS limitation"
instruction is right, and it is verifiable:

| Leg | Backend | Where |
|-----|---------|-------|
| SMTP (outbound / reply) | **rustls** | `Cargo.toml:11` — `lettre` with `tokio1-rustls-tls`, `default-features = false` |
| IMAP (inbound) | **native-tls** | `imap.rs:222` — `native_tls::TlsConnector::new()` |

Feature resolution for the SMTP leg, traced through lettre's own `Cargo.toml`:
`tokio1-rustls-tls` → `tokio1-rustls` + `rustls-tls`; `rustls-tls = ["webpki-roots", "rustls", "ring"]`.
`rustls-platform-verifier` is **not** in `Cargo.lock` at all; `rustls-native-certs` is
present but pulled by other crates, not enabled as a lettre feature. So
`CertificateStore::Default` falls through both `#[cfg]` arms to `load_webpki_roots()` —
the **compiled-in Mozilla bundle**.

**Consequences, and these are the platform-coverage claim for Task 1:**

1. The SMTP trust anchors are compiled into the binary. The path consults **no**
   platform trust store — not the macOS keychain, not the Windows cert store, not
   `/etc/ssl` on Linux.
2. Therefore `SSL_CERT_FILE` is inert on the SMTP path on **every** platform. That is a
   *stronger and different* statement than the prior lane's macOS IMAP finding, which was
   specifically that Security.framework ignores `SSL_CERT_FILE`. Same symptom, different
   mechanism, different platform scope. I must not merge the two in the summary.
3. `add_root_certificate` is consequently the **only** way to introduce a private root
   into the SMTP path, on any OS.
4. Because the root set is compiled in rather than sourced from the OS, the SMTP leg's
   TLS behaviour is **platform-uniform**. A Linux proof of this leg does generalise to
   macOS and Windows — unlike the IMAP leg, where it would not.

### E4. Wiring surface is a single call site

`/usr/bin/grep -rn "LettreSender::new" crates/` → exactly one hit,
`crates/wcore-channel-email/src/lib.rs:264`. (Instrument liveness: the same grep for
`LettreSender` returns the struct def, the impl block and the call — non-zero, so the
single-hit result is a real count, not a dead-tool zero.)

`SmtpConfig` (`config.rs:32-41`) carries `host`, `port`, and two credential handles.
It is `#[serde(deny_unknown_fields)]`, so a new key is additive but strictly
additive-only.

Planned change (smallest that closes the leg):
- add `tls_root_cert_path: Option<String>` to `SmtpConfig`, `#[serde(default)]`;
- `LettreSender::new` takes it, and when `Some`, reads the PEM and builds
  `TlsParameters::builder(host).add_root_certificate(Certificate::from_pem(&bytes)?)`,
  then `.tls(Tls::Required(params))` to override the default params that
  `starttls_relay` installs (async_transport.rs:169-177 shows it sets
  `Tls::Required(TlsParameters::new(relay))`, so overriding is required, not optional);
- `accept_invalid_certs` stays untouched and unreachable from config. Explicitly NOT
  adding it — a prior lane refused it as a production security regression and that
  judgement is being upheld.

---

## T+1:40 — the gate reddened on its own, against MY code. Finding E5.

Baseline before the feature: `cargo test -p wcore-channel-email` → **80 passed; 0 failed;
0 ignored; 0 measured; 0 filtered out**. Real executed count, not an exit status.

After adding the anchor tests, run at `53e5dca7`:

```
test result: FAILED. 82 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
---- smtp::tests::tls_root_cert_path_with_garbage_contents_errors stdout ----
panicked at crates/wcore-channel-email/src/smtp.rs:530:13:
a non-PEM anchor file must not be ignored
```

### E5 (HIGH, in my own change — now fixed): `Certificate::from_pem` accepts non-PEM input

`build_tls_params` was handed a file containing `this is not a certificate\n` and returned
**Ok**. Root cause read in lettre 0.11.22 `tls.rs:675-698`:

```rust
#[cfg(feature = "rustls")]
let rustls_cert = {
    CertificateDer::pem_slice_iter(pem)
        .collect::<Result<Vec<_>, pki_types::pem::Error>>()
        .map_err(|_| error::tls("invalid certificates"))?
};
```

`pem_slice_iter` yields **nothing** — not an error — for bytes containing no PEM block, so
the `collect` succeeds into an empty `Vec` and `from_pem` returns
`Ok(Certificate { rustls: vec![] })`. `add_root_certificate` then stores a certificate
carrying zero anchors, and lettre's `for rustls_cert in cert.rustls` loop (tls.rs:518-526)
iterates zero times.

**Impact had this shipped:** an operator pointing `tls_root_cert_path` at a private key, an
empty file, or a typo'd-but-existing path would get a sender that builds cleanly, silently
trusts only the default roots, and then fails at send time with an opaque TLS error. That is
exactly the silent-degradation mode the test's own doc comment predicted — and it was live in
my code until the test caught it.

Note the asymmetry: `from_pem` on lettre's **native-tls** path *does* reject such input
(`native_tls::Certificate::from_pem` errors). This crate builds lettre with rustls, so it
does not. A reviewer reasoning from the native-tls behaviour would have concluded, wrongly,
that the input was already validated.

**Fix:** `build_tls_params` now counts the anchors with the same parser lettre uses
(`CertificateDer::pem_slice_iter`, via a direct `rustls-pki-types` dep already present
transitively) and refuses an empty set with an error naming the file.

**This is the redden proof for Task 1's gate.** It was not manufactured — the gate failed
on first execution, against real code, for a real defect, and identified the defect
precisely. Per §6b-ii the instrument was repaired in-lane rather than written up and left.

---

## T+2:10 — Task 2. The inherited msteams costing is wrong on BOTH counts.

Written down before building anything, per §6b-i.

### E6. msteams is NOT "discord-shaped"

Discord's cost came from a **persistent WebSocket gateway**. msteams inbound is an
**HTTP webhook** carrying a **Bot Framework JWT** — asymmetric, issuer/audience-bound.
Those are different transports with different work. The inherited "discord-shaped,
~2 sessions" estimate is not a description of this connector.

### E7. The README claim is STALE — msteams inbound is already implemented AND routed

`README.md:348` says MS Teams is a send-only MVP whose inbound "is parsed but not yet
exposed over the host." Both halves fail against current source.

**It is implemented.** `crates/wcore-channel-msteams/src/lib.rs:249` overrides
`Channel::ingest_webhook` with real authentication, not a stub:

```rust
let claims = auth.validate(hdr).await.map_err(|e| ChannelError::Auth(e.to_string()))?;
```

plus a defense-in-depth check that the JWT's `serviceurl` claim matches the Activity's
`serviceUrl` (blocking replay with a swapped serviceUrl, which matters because the reply
path trusts that URL). Two auth tests already exist at lib.rs:705 and lib.rs:723.

**It is routed.** The host has **no platform allow-list**. Read in full at
`crates/wcore-agent/src/inbound_webhook.rs:142-202`, the handler is entirely generic:

```rust
async fn handle_webhook(State(state): State<HostState>, Path(channel): Path<String>, ...) -> Response {
    ...
    let result = state.manager.read().await.ingest_webhook(&channel, &req).await;
    response_for(&channel, result)
}
// route: "/webhooks/:channel"
```

So `POST /webhooks/msteams` dispatches to the msteams channel by name. Nothing filters it.

Query for the absence-of-allow-list claim, so it can be re-run:
`/usr/bin/grep -n "slack\|whatsapp\|sms\|msteams\|platform\|route\|path" crates/wcore-agent/src/inbound_webhook.rs`
— instrument liveness confirmed in the same sweep: `grep -rl "slack" --include='*.rs' crates/`
returns **54** files, and the ingest_webhook census returns 13 sites across 5 crates, so the
tooling is alive. The allow-list absence is additionally backed by *reading the whole
handler*, not by a grep returning zero.

### E8. What is stale, precisely

Three places state the old world, and the bootstrap comment states it as the REASON the
host is safe — so it is load-bearing, not cosmetic:

| Site | Stale text |
|------|-----------|
| `README.md:348` | "MS Teams is a send-only MVP; inbound is parsed but not yet exposed over the host" + "The inbound webhook host serves Slack, WhatsApp, and Twilio SMS only" |
| `crates/wcore-agent/src/bootstrap.rs:3352-3355` | "only the signature-verified connectors override the trait method, so msteams' unauthenticated parse stays unexposed" |
| `crates/wcore-channels/src/lib.rs:288-294` | default-impl doc: "Webhook connectors that verify the platform signature (Slack, WhatsApp, Twilio SMS) override this" |

### E9. The three-state discriminator this gives me

`ingest_webhook`'s default impl (`wcore-channels/src/lib.rs:299`) returns
`ChannelError::Rejected`, and `response_for` maps errors to distinct statuses
(inbound_webhook.rs:119-140). So one HTTP call separates all three worlds:

| status | meaning |
|--------|---------|
| **404** | channel not configured (`ChannelError::Config`) |
| **400** | reached the DEFAULT trait impl — i.e. genuinely unexposed |
| **401** | reached **msteams' own JWT validation** — i.e. exposed |

A 401 on an unauthenticated `POST /webhooks/msteams` is therefore positive proof of
exposure that cannot be confused with "not configured" or "unexposed". That is the gate
worth building, and it is far cheaper than the inherited estimate.

### Determination (pre-build)

The inbound clause is **not 2 sessions of build** — it is already built. The real remaining
work is (a) a host-level regression test asserting the 401, and (b) correcting the three
stale sites. I am building both. If the test does NOT return 401, then the README is right
and I am wrong — and I will report that instead.
