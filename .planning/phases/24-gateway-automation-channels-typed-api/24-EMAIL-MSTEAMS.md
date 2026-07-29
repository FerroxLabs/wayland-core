# 24-EMAIL-MSTEAMS — lane report

Branch `lane/24-email-msteams`, base `e77b44b0` (verified equal to
`gh/plan/f20-unified-audit-repair` at fetch time). Working notes and the
measurement trail: `24-EMAIL-MSTEAMS-NOTES.md` (committed at T+15 per §6b-i and
appended after each measurement).

**Verdict: both tasks achieved.** Task 1 closed the email reply half and, in doing
so, the new gate caught a real defect in the change itself. Task 2's inherited
costing turned out to be wrong on both shape and size — the msteams inbound is
already built and already exposed, so the deliverable became a proof and a
documentation correction rather than a build.

---

## Task 1 — email reply half: CLOSED, with counts

### What landed

`smtp.tls_root_cert_path` (optional, `#[serde(default)]`) now flows
config → `EmailChannel::start` → `LettreSender::new` → lettre's TLS parameters.

`LettreSender::new` previously built the transport as
`starttls_relay(host).port(port).credentials(creds).build()` and passed **no TLS
parameter at all** — the brief's diagnosis was exactly right. Because
`starttls_relay` has already installed `Tls::Required(TlsParameters::new(host))`
(lettre `async_transport.rs:169-177`), an extra anchor can only be introduced by
replacing those parameters wholesale, so the fix is an explicit
`.tls(Tls::Required(..))` override rather than an amendment.

### The determination was verified, not inherited

The brief's claim about `add_root_certificate` was re-read in lettre's own source
at the resolved version (0.11.22, pulled from static.crates.io):

* `add_root_certificate` (`tls.rs:248`) only pushes onto `root_certs`.
* The build loop (`tls.rs:518-526`) adds those anchors on top of the base store.
* The verifier-replacing branch is gated **solely** on
  `accept_invalid_certs || (extra_roots.is_none() && accept_invalid_hostnames)`.

So the reply half is reachable with chain and hostname verification fully intact.
`accept_invalid_certs` is **not** offered and is unreachable from config — the
earlier lane's refusal of it stands, and this lane did not quietly reintroduce it.

### Platform coverage — and why it is NOT the macOS IMAP limitation

The two email legs run on **different TLS backends**, which is the concrete reason
the brief's "do not conflate these" instruction is correct:

| leg | backend | evidence |
|-----|---------|----------|
| SMTP (reply) | **rustls** | `Cargo.toml:11` — lettre, `default-features = false`, `tokio1-rustls-tls` |
| IMAP (inbound) | **native-tls** | `imap.rs:222` — `native_tls::TlsConnector::new()` |

Tracing lettre's feature graph: `tokio1-rustls-tls` → `rustls-tls` →
`["webpki-roots", "rustls", "ring"]`. `rustls-platform-verifier` is absent from
`Cargo.lock` entirely and `rustls-native-certs` is not enabled as a lettre feature,
so `CertificateStore::Default` falls through to `load_webpki_roots()` — the
**compiled-in Mozilla bundle**.

Consequences, stated precisely:

1. The SMTP path reads **no OS trust store on any platform** — not the macOS
   keychain, not the Windows store, not `/etc/ssl`.
2. Therefore `SSL_CERT_FILE` is inert on the SMTP path **everywhere**. This is a
   *different and stronger* statement than the prior lane's macOS finding, which was
   that Security.framework ignores `SSL_CERT_FILE` on the **IMAP** leg. Same
   symptom, different mechanism, different scope.
3. `tls_root_cert_path` is consequently the only way to introduce a private anchor
   into the SMTP path, on every OS.
4. Because the anchors are compiled in rather than sourced from the platform, this
   leg's TLS behaviour is **platform-uniform**, so the Linux proof below
   **does generalise to macOS and Windows**. The IMAP leg's would not — and this
   lane did not touch, test, or claim anything about IMAP.

**Coverage claim: Linux (proven, hetzner), macOS and Windows (covered by the same
compiled-in root store and the same rustls code path; not separately executed).**
The Darwin-behaviour exception in §0 was **not** used — nothing here is
Darwin-specific.

### Counts

All on `hetzner-dsm`, targeted per-crate runs, at `feba8f2a`, clean tree:

| run | result |
|-----|--------|
| baseline before the feature | `80 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `wcore-channel-email` lib, final | `83 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `smtp_tls_root_live` integration, final | `1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `cargo clippy -p wcore-channel-email --all-targets` | exit 0, no warnings for this crate |

Executed counts are read back, not inferred from exit status. The live test is
**not** `#[ignore]`d, not env-gated and needs no external host — it mints its own
CA and relay in-process, so it runs in a normal `cargo test`.

### Live evidence — arrivals proven positively

`tests/smtp_tls_root_live.rs` stands up a real STARTTLS SMTP relay on a loopback
socket presenting a certificate from a throwaway CA, then drives the same
`LettreSender` twice, changing one variable:

* **root wired** → relay message count goes **0 → 1**, and the delivered payload is
  asserted to contain this test's own marker (`wayland-core-smtp-tls-root-proof-8f31c2`),
  so "delivered" is tied to this send rather than to any traffic.
* **no root** → send is refused, relay count **stays 1**. Recorded refusal:
  `transient: Connection error: Connection error: invalid peer certificate: UnknownIssuer`

The negative control is load-bearing: had the leg been "fixed" with
`accept_invalid_certs`, the unrooted run would have **succeeded**. Its failure is
therefore executable proof that certificate verification is still enabled.

### The gate was proven able to redden — three ways, none manufactured

1. **It reddened on its own, against my code.** See the HIGH finding below.
2. **Mutation of the live gate.** Dropping the `.tls(...)` wiring while keeping the
   validation (reproducing the original bug) →
   `test result: FAILED. 0 passed; 1 failed` with
   `send with tls_root_cert_path wired must reach the relay, got: Some(Transient("… UnknownIssuer"))`.
   Restored → `1 passed`.
3. Known-positive/known-negative pairing inside the config tests.

### HIGH finding, in this lane's own change — found and fixed in-lane

**`lettre::…::Certificate::from_pem` accepts input containing no certificate.**
On the rustls path it is
`CertificateDer::pem_slice_iter(pem).collect::<Result<Vec<_>, _>>()`
(`tls.rs:675-698`), and that iterator yields **nothing** — not an error — for bytes
with no PEM block. The collect therefore succeeds and returns
`Ok(Certificate { rustls: vec![] })`. `add_root_certificate` then stores a
certificate carrying **zero** anchors and lettre's `for rustls_cert in cert.rustls`
loop iterates zero times.

Impact had it shipped: an operator pointing `tls_root_cert_path` at a private key,
an empty file, or a typo'd-but-existing path would get a sender that builds
cleanly, silently trusts only the default roots, and fails much later with an
opaque TLS error. Note the asymmetry that makes this easy to miss — lettre's
**native-tls** path *does* reject such input, so reasoning from native-tls
behaviour gives the wrong answer for this build.

Caught by `tls_root_cert_path_with_garbage_contents_errors` on its first execution
(`82 passed; 1 failed`). Fixed by counting the anchors with the same parser lettre
uses (`rustls-pki-types`, already resolved transitively) and refusing an empty set
with an error naming the file. Per §6b-ii the instrument was repaired in-lane, not
written up and left.

---

## Task 2 — msteams: costing was wrong on BOTH counts

### The inherited estimate does not describe this connector

"~2 sessions, discord-shaped" fails on shape first: Discord's cost came from a
persistent **WebSocket gateway**. msteams inbound is an **HTTP webhook** carrying a
**Bot Framework JWT** — asymmetric, issuer/audience-bound. Different transport,
different work. Verified from source, as instructed, rather than inherited.

### `README.md:348` is STALE — checked, as the brief asked

Both halves of the claim fail against current source:

**It is implemented.** `crates/wcore-channel-msteams/src/lib.rs:249` overrides
`Channel::ingest_webhook` with real authentication, not a stub — `auth.validate(hdr)`
plus a defence-in-depth check binding the JWT's `serviceurl` claim to the Activity's
`serviceUrl` (which matters because the reply path trusts that URL). The crate's own
suite already covers wrong signature, wrong audience, wrong issuer and expired token.

**It is exposed.** The host holds **no per-platform allow-list**. Read in full at
`crates/wcore-agent/src/inbound_webhook.rs:142-202`: the handler is generic over
`Path(channel)` and its only dispatch is
`state.manager.read().await.ingest_webhook(&channel, &req)`, on route
`/webhooks/:channel`. Safety comes from the *trait default* returning `Rejected`,
not from a list. So `POST /webhooks/msteams` reaches msteams' authenticated ingest.

This absence claim is backed by reading the whole handler, not by a grep returning
zero; the greps used were stated in the notes and their instrument liveness
confirmed in the same sweep (`grep -rl "slack" --include='*.rs' crates/` → **54**
files; the `ingest_webhook` census → 13 sites across 5 crates). Globs were quoted —
an unquoted `--include=*.rs` was eaten by zsh earlier in this lane, exactly the
§3b-i trap.

### Proven executably, not just by reading

Added `manager_dispatch_reaches_msteams_authenticated_ingest_not_the_default_impl`,
which drives `ChannelManager::ingest_webhook("msteams", req)` — the host's own
dispatch — and discriminates three states by error variant, which is precisely what
the host maps to a status in `response_for`:

| variant | status | meaning |
|---------|--------|---------|
| `Config` | 404 | channel not configured |
| `Rejected` | 400 | fell through to the DEFAULT trait impl → **unexposed** |
| `Auth` | 401 | msteams' own JWT validation ran → **exposed** |

The test asserts `Auth` and explicitly **not** `Rejected`, with an unknown-channel
known-negative in the same test so the assertion is not one every input satisfies.

`wcore-channel-msteams`: **34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out.**

**Redden proof.** Injecting the exact unexposed behaviour (making the override
return what the default impl returns) →
`test result: FAILED. 0 passed; 1 failed` with
`host dispatch must reach msteams' JWT validation (-> 401); got Rejected("channel does not accept inbound webhooks")`.
Restored → `1 passed`. A first mutation attempt produced only a **compile** error,
which is not a behavioural red; it was discarded and redone rather than counted.

### Determination

**msteams inbound did not need building — it needed proving and documenting.**
Against the brief's fork ("if ~2 sessions build it; if materially more, land the
seam and cost the fixture"), neither branch applied: the work was already done and
the estimate was measuring a connector that does not exist in this shape.

Remaining genuinely-unbuilt msteams work, costed from source:

| item | cost | note |
|------|------|------|
| inbound over the host | **already done** | proven above |
| inbound **attachments** | ~1 session | `inbound.rs:19-21` — Teams delivers `attachments[]` as `contentType`/`contentUrl`; fetching needs a separate auth-gated Graph/Connector download. This is the real open gap, and it is the only one. |

Three stale sites corrected (the bootstrap one was **load-bearing** — it stated the
staleness as the *reason the host is safe*):

* `README.md:348`
* `crates/wcore-agent/src/bootstrap.rs:3352` (comment)
* `crates/wcore-channels/src/lib.rs:288` (default-impl doc)

---

## Gates, all read back

At `feba8f2a`, hetzner, clean tree (`git status --porcelain` → 0 lines), targeted
per-crate runs (never a full-workspace run under lane contention):

```
wcore-channel-email  lib   83 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
smtp_tls_root_live         1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
wcore-channel-msteams     34 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
wcore-channels           114 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
wcore-channels (int)      17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
clippy -p wcore-channel-email --all-targets   exit 0, no warnings for this crate
cargo fmt --all                                clean (run on the Mac, permitted)
```

`0 ignored` and `0 filtered out` are asserted deliberately: a suite that exits 0
having run zero tests is a known self-passing shape in this repo.

---

## What I did NOT do

* Did **not** touch `scripts/f24-inbound.mjs`, `crates/wcore-channel-matrix/`,
  `.github/workflows/ci.yml`, `crates/wcore-cli/src/{lib,main}.rs`, or
  `.planning/BACKLOG.md`. The wcore-cli shared fence was never edited, so no fence
  diff is needed.
* Did **not** merge, open a PR, tag, release, close an issue, or run
  `wcore-contract generate`.
* Did **not** test or change the **IMAP** leg; the macOS Security.framework
  limitation there is untouched and unclaimed.
* Did **not** run a full-workspace build, clippy on the Mac, or use the
  Darwin-behaviour exception.
* Did **not** use any real credential; nothing needed one. The email proof runs
  against a self-minted CA and a loopback relay.
* Did **not** build msteams inbound attachments (costed above at ~1 session).
* Did **not** run `wcore-agent`'s test suite — only `cargo check -p wcore-agent
  --lib` (green, exit 0, `Finished dev profile in 51.32s`), which is what the
  comment-only `bootstrap.rs` edit requires. The pre-existing `imap-proto v0.10.2`
  future-incompat warning is not mine and was present at base.

## For the orchestrator to serialize

* **`Cargo.lock` is modified** (+132 lines, dev-only: the `rcgen` chain for the
  email TLS relay test). Expect a lock conflict with any concurrently-merging lane;
  resolve by regenerating rather than hand-merging.
* New **dev-dependencies** on `wcore-channel-email`: `rcgen`, `rustls`,
  `tokio-rustls`, `lettre`, `tempfile`. New **runtime** dep: `rustls-pki-types`
  (already resolved transitively — adds no new crate to the graph).
* Shared files touched outside my crates, all minimal: `README.md` (one paragraph),
  `crates/wcore-agent/src/bootstrap.rs` (comment only),
  `crates/wcore-channels/src/lib.rs` (doc comment only).
* No protocol seam or contract change requested.
