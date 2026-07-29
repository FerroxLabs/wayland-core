# 24-C3-FINISH — running notes (append-only, committed per §6b-i)

Lane `lane/24-c3-finish`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-c3-finish`.
Merge-base captured once at start: **`8bcb052b2aa6b1a9e3f2ed00af935a58c92c1f11`**
(= `plan/f20-unified-audit-repair` at fetch time). Every fence diff in this lane is
taken against that SHA, never against the branch name.

---

## T+0 — what I inherited, read from the four prior summaries

The criterion (`ROADMAP.md:119`) has **eight clauses**:

> setup/auth, access, routing, media, native actions, idempotency, reconnect/reload, health

State on arrival, per clause, aggregated across the five driven adapters:

| clause | state on arrival | proven on |
|---|---|---|
| setup/auth | PROVEN | slack, whatsapp, sms, telegram, discord (fixtures mint + ENFORCE their own token) |
| access | PROVEN | same five (telegram/discord with positive admit controls) |
| routing | PROVEN | same five |
| media | **UNTOUCHED** | — |
| native actions | **UNTOUCHED** | — |
| idempotency | PROVEN (inbound dedupe) | same five |
| reconnect/reload | **UNTOUCHED** | — |
| health | **UNTOUCHED** | — |

So **4 of 8 clauses are untouched on the inbound path, for every adapter.** That is the
single largest remaining block, and it is bigger than the adapter axis: adding signal or
matrix would add adapters to clauses already proven, not new clauses.

Adapter axis on arrival:

| adapter | transport | driven? | legs |
|---|---|---|---|
| slack | webhook | yes | 5/5 |
| whatsapp | webhook | yes | 5/5 |
| sms | webhook | yes | 5/5 |
| telegram | poll | yes | 5/5 |
| discord | websocket | yes | 6/6 incl. steady |
| email | poll (IMAP) | HALF — admission proven, reply half blocked at TLS | route/bind NOT MEASURED |
| signal, matrix, msteams, imessage | ? | **never driven** | — |

## T+0 — priority chosen, and why

Ordered by clauses-closed-per-session, with the two cheap determinations pulled forward
because they are deliverables in themselves (§6b-i: a measured refusal is a result) and
because one of them may collapse several adapters onto one seam shape:

1. **P1 — seam survey for signal / matrix / msteams / imessage.** Pure source read, cheap.
   Telegram's seam cost *zero* Rust (the field already existed); Discord's cost two config
   fields and ~3 sessions because its transport is a WebSocket. Establishing which of the
   four are telegram-shaped vs discord-shaped is high-information per minute, and the brief
   explicitly asks for the cost estimate rather than the build.
2. **P2 — email reply-half reachability determination.** The brief is explicit: establish
   whether it is reachable, and if not say what it would take, rather than forcing it. Two
   facts are already MEASURED and must not be re-derived or conflated:
   - Linux/OpenSSL honours a child-scoped `SSL_CERT_FILE` for IMAP — that already worked;
   - macOS `native-tls` = Security.framework — it does not;
   - **SMTP on EVERY platform resolves to `webpki-roots`** — compiled-in, reads no file and
     no env var. Proven executably (IMAP accepted a cert, SMTP refused the identical one
     0.6 s later, 82 identical sessions).
   My job is the *third* question those two leave open: is there a config-level or
   dependency-level knob, and what is the exact cost of adding one.
3. **P3 — re-measure `gateway run`.** Cheap, and re-validates the lease + starvation fixes
   that landed overnight and have not been exercised on the gateway surface since. Doing it
   early de-risks everything downstream: if `gateway run` regressed, every json-stream
   figure I take afterwards is measuring the wrong surface.
4. **P4 — the four untouched clauses** (media, native actions, reconnect/reload, health).
   Biggest clause win, largest build. Taken last because P1-P3 are bounded and P4 is not.

**I will not record 24-C3 as MET unless every clause genuinely is.** Four lanes before me
correctly declined. On present state that is the near-certain outcome again, and saying so
early is not defeatism — it is the thing that stops a premature MET on the last release
blocker.

## T+0 — traps I am carrying forward (from the brief and the prior summaries)

- **A green can be manufactured by universal denial.** The `access` leg once passed on all
  three webhook adapters *because everything was denied*. Every zero I report must be paired
  with a positive admit control **inside the pass condition**, not merely printed beside it.
  A green with zero arrivals grades FAIL.
- **Instruments carry the defect they hunt — 20+ recorded instances**, at least four of them
  in this criterion's own harnesses. Two had opposite sign: one under-counted 8 real arrivals
  as `replied=0` (MarkdownV2 escaping mangled the correlation token), one over-counted a
  duplicate that was really the driver's own 90 s latency against a 60 s TTL. Any suspect run
  grades **INCOMPLETE via an explicit `instrument_fault` state, never LOSS**, and every
  instrument repair carries the three-assertion self-test (known-positive passes,
  known-negative fails, **and the old matcher would have missed it**).
- **§6b-ii: repair the instrument in the lane that finds the defect.** A written-up
  instrument defect is a defect you have agreed to keep; that exact sequence recurred once
  already on this program.
- **Byte-count every capture.** `${PIPESTATUS[0]}` after a pipeline returns empty on this
  host. Capture exit status on the line after the command.
- **Run test targets by file, never by filter.** `cargo test -p wcore-cli migrate` exited 0
  having run 0 tests. Always read the `N passed` count back.
- Contention on hetzner: a full-workspace run while other lanes build is not a measurement.
  Re-run the crate alone at the same commit before reporting any regression.

## T+0 — open questions this lane must answer

1. Do signal / matrix / msteams / imessage have a config-level base-URL seam already? If
   not, is each telegram-shaped (one field, ~0 Rust) or discord-shaped (two fields + a
   protocol fixture, ~3 sessions)?
2. Is the email reply half reachable *at all* as currently built? If not, what is the
   minimum change — a cert-source knob on the adapter, a feature-flag swap of
   `webpki-roots` → `rustls-native-certs`, or a publicly-chained cert?
3. Does `gateway run` still receive inbound after the lease + starvation fixes?
4. What do media / native actions / reconnect-reload / health even MEAN on the inbound path,
   and which are measurable with the fixtures that already exist?

---

## T+35 — P1 RESULT: the seam survey. Two of the four are drivable TODAY with zero Rust.

Source-level, read from the shipped construction path (`wcore-channels-registry`'s
`make_*` → the adapter's `new()`), not from the `#[doc(hidden)]` test constructors. That
distinction is the whole point: Discord's seam *existed* as `with_bases` and was still
unreachable, because the registry built through `new()`. So the only question that matters
is **what `new()` consumes from config.**

| adapter | inbound transport | seam reachable from a config file? | Rust change needed | shape |
|---|---|---|---|---|
| **matrix** | HTTP long-poll `/sync` | **YES — `homeserver_url`** | **ZERO** | telegram-shaped, but *stronger* |
| **signal** | stdio JSON-RPC subprocess | **YES — `signal_cli_path`** | **ZERO** | not a URL seam at all |
| msteams | inbound webhook + outbound REST | PARTIAL — `service_url` yes, `token_url` NO | 1–2 config fields | discord-shaped |
| imessage | SQLite poll + AppleScript send | env-only (`$HOME`), and macOS-gated | send path has no seam | blocked on platform |

### matrix — `homeserver_url`, and it is a *required* field

`MatrixConfig.homeserver_url` (`config.rs:9`) has **no `#[serde(default)]` and no production
constant**. `MatrixChannel::new` (`lib.rs:61-62`) does
`let api_base = config.homeserver_url.clone(); Self::with_base(...)`, and
`registry::make_matrix` (`lib.rs:179`) calls `new()`. So a config naming
`homeserver_url = "http://127.0.0.1:PORT"` points the **shipped binary** at a fixture.

This is a stronger seam than telegram's, and the difference is worth stating precisely.
Telegram, slack, whatsapp, sms and msteams all carry a `#[serde(default)]` production
endpoint, so a fixture run must *override* a default that would otherwise reach the vendor —
and the control test ("a config naming no override still reaches production") is load-bearing.
Matrix has no default to reach past: every matrix config that has ever parsed already names
its homeserver, because a homeserver is per-deployment by design. **There is no production
default to preserve, so there is no control test to write and no regression surface.**

Egress is not a blocker: matrix uses the same `wcore_egress::EgressClient` as telegram,
slack, discord and whatsapp, all of which have been driven against loopback fixtures.

**Cost: fixture only.** The `/sync` long-poll is the same shape as telegram's `getUpdates`
fixture that already exists in `scripts/f24-tg-fixture.mjs` — an HTTP endpoint that holds a
request open and returns a batch with a cursor. Nothing like Discord's hand-rolled RFC6455.

### signal — `signal_cli_path`, and it is not a network seam at all

`SignalConfig.signal_cli_path` (`config.rs:18`, `#[serde(default = "default_signal_cli_path")]`
→ bare `signal-cli` on `$PATH`). `RealLauncher::launch` (`subprocess.rs:53-62`) does
`Command::new(cli_path).arg("-a").arg(account).arg("jsonRpc")` with piped stdio.
`registry::make_signal` (`lib.rs:166`) calls `new()`.

So the fixture is **an executable script that speaks JSON-RPC frames on stdin/stdout.** No
HTTP, no TLS, no port, no certificate — categorically simpler than every other adapter's
fixture, including the webhook ones. The seam is a `PathBuf` in operator-owned config, at the
same trust level as `credential_handle`; it is not reachable from a message.

**This breaks the framing I was given.** The brief said the telegram and discord precedents
are both "config-level base-URL seams". Signal's is a config-level **subprocess-path** seam.
Anyone searching the four remaining adapters for a `*_base_url` field would have found
nothing in signal and concluded it had no seam — when in fact it has the cheapest one in the
entire set.

### msteams — discord-shaped, and the blocker is the token endpoint, not the service URL

Two endpoints, and only one is seamed:

- outbound `service_url` — `#[serde(default)]` config field, present (`config.rs:17`). Fine.
- **AAD token** `BF_TOKEN_URL` = `https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token`
  (`token.rs:12-13`). `MsTeamsChannel::with_token_url` exists but is **`#[doc(hidden)]`**
  (`lib.rs:82`) and `new()` hardcodes the constant (`lib.rs:78`); `make_msteams`
  (`registry:192`) calls `new()`.

This is **precisely the Discord situation**: the seam exists in Rust and no out-of-process
harness can reach it. Consequence: msteams cannot even *start* against a fixture, because
`start()` must acquire an app token from Microsoft before anything else happens. A third
hardcoded endpoint, `BF_OPENID_METADATA_URL` (`auth.rs:42-43`), is needed for inbound JWT
validation — `BotFrameworkAuth` takes it as a constructor arg (`auth.rs:115`), so it is
plumbed but likewise not config-reachable.

**Cost: 1–2 `#[serde(default)]` config fields + a control test**, then a webhook fixture that
also mints a JWKS. The Rust change is telegram-sized; the fixture is larger than matrix's
because it must sign JWTs. Call it ~2 sessions.

### imessage — the send path has no seam, and the platform blocks it anyway

- Read path: `chat_db_path()` (`db.rs:26-32`) is a **free function taking no argument**,
  called directly at `channel.rs:156` and `:223`. It reads `$HOME` and appends
  `Library/Messages/chat.db`. So there *is* an env-level seam for the inbound half — point
  `$HOME` at a temp dir holding a fixture SQLite file with chat.db's schema.
- Send path: `applescript.rs` shells to `osascript`. macOS-only, and gated behind TCC
  Automation consent that a headless run cannot grant.
- The whole crate is `#[cfg(target_os = "macos")]` (`lib.rs:3`, and `make_imessage` is
  `#[cfg(target_os = "macos")]` at `registry:199`).

So iMessage is **not blocked on a seam — it is blocked on the platform.** It cannot be built
on hetzner (cfg'd out) and cannot be built on the Mac (standing constraint: no cargo on the
Mac). A macOS leg needs the CI-published `wayland-core-aarch64-apple-darwin` artifact, and
even then the reply half needs TCC consent. **Do not cost this as a fixture problem.**

### What this changes about item 2

The four were not equidistant, and were being treated as if they were. Two require **no Rust
change at all** and one of those two (signal) has the simplest fixture in the phase. The
correct order for anyone continuing is **matrix → signal → msteams → imessage**, and the
first two do not need a seam plan, only a fixture.

## T+55 — P2 RESULT: the email reply half IS reachable. Not from config — but the knob exists.

**This changes the inherited conclusion, and the change is in the product's favour.** I was
briefed that "the email *reply* half may not be reachable by fixture at all as currently
built". The two measured facts I was told not to re-derive are both correct and I did not
re-derive them. But they answer a narrower question than the one that was drawn from them.

### What was already established, and is not in dispute

- Linux/OpenSSL honours a child-scoped `SSL_CERT_FILE` for **IMAP**; macOS `native-tls` =
  Security.framework and does not. (`imap.rs:194` → `native_tls::TlsConnector::new()`.)
- **SMTP on every platform resolves to `webpki-roots`, which reads no file and no env var.**
  Confirmed again here from the lockfile: `lettre 0.11.22`'s resolved dependency list contains
  **`webpki-roots 1.0.7`** and contains **no `rustls-native-certs`** and no
  `rustls-platform-verifier` (`Cargo.lock:4231-4255`).

I confirmed the resolution path in lettre's own source rather than inferring it. With the
feature set this workspace actually resolves, `TlsParametersBuilder::build` takes the branch at
`tls.rs:505-510`:

```rust
#[cfg(all(not(feature = "rustls-platform-verifier"),
          not(feature = "rustls-native-certs"),
          feature = "webpki-roots"))]
load_webpki_roots(&mut root_cert_store);
```

So `CertificateStore::Default` → Mozilla's compiled-in set, exactly as briefed. **No env var
reaches it. That half of the finding is solid and I am not softening it.**

### The part that was not established: `add_root_certificate` works under rustls

`tls.rs:518-526`, and this loop runs **unconditionally, after and independent of the
`cert_store` match**:

```rust
for cert in self.root_certs {
    for rustls_cert in cert.rustls {
        root_cert_store.add(rustls_cert).map_err(error::tls)?;
    }
}
```

`TlsParametersBuilder::add_root_certificate` (`tls.rs:248`) and
`TlsParameters::builder(domain)` (`tls.rs:603`) are both public and both available with this
crate's resolved features. **An extra root can be added ON TOP of webpki-roots without
removing webpki-roots and without disabling verification.**

### Why the adapter cannot use it today

`LettreSender::new` (`smtp.rs:71-84`) is the entire SMTP construction path:

```rust
let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)?
    .port(port).credentials(creds).build();
```

It passes **no TLS parameter at all**, so lettre supplies the default, which is
`CertificateStore::Default` with an empty `root_certs`. There is no cert-source argument, no
`EmailConfig` field, and no `#[doc(hidden)]` alternate constructor. This is **not** the Discord
situation (seam exists, unreachable); it is a seam that does not exist yet.

### Determination

**Reachable: YES — with a bounded Rust change. Reachable from configuration alone: NO.**

Minimum change, mirroring the `TelegramConfig::api_base_url` precedent exactly:

1. `EmailConfig` gains `#[serde(default)] smtp_tls_root_cert_path: Option<PathBuf>`. A missing
   field is not an unknown field, so every existing config still parses under
   `deny_unknown_fields`.
2. `LettreSender::new` takes it. `None` → the existing `starttls_relay(host)` call, byte for
   byte. `Some(p)` → `TlsParameters::builder(host).add_root_certificate(Certificate::from_pem(..))`
   passed via `.tls(Tls::Required(params))`.
3. The **control test is the load-bearing one**, as it was for telegram and Discord: a config
   naming no cert path must reach production SMTP with production trust, unchanged.

Estimated cost: one config field, ~15 lines in `smtp.rs`, two tests. **~1 session.**

### The security trap in this change, recorded because it is the obvious shortcut

`TlsParametersBuilder` also exposes `dangerous_accept_invalid_certs` (`tls.rs:313`) and
`dangerous_accept_invalid_hostnames` (`tls.rs:279`). Either one would make the fixture run go
green in one line.

**Both must be refused.** `add_root_certificate` ADDS a trust anchor the operator chose;
`dangerous_accept_invalid_certs` REMOVES verification for everyone, in production, permanently.
Reaching for it would be shipping a real security regression to make a test pass — the most
dangerous form of "weaken a test to reach green", because it lands in the product rather than
in the harness. The cert-path knob is operator-owned configuration at the same trust level as
`credential_handle`, and it is not reachable from a message.

### Decision: determined, NOT built this lane — and why

The brief said establish reachability rather than force it, and I have. I am not building it
here, on two grounds:

1. **Clause math.** Email `route`/`bind` would add a sixth adapter to `routing`, a clause
   already proven on five. The four untouched clauses are worth strictly more per session.
2. **It touches the production TLS path.** 24-C3-H2 declined to fix F24-C3-H4 blind at the end
   of a lane on the reasoning that "fixing it blind, at the end of a lane, is how a fix becomes
   the next lane's defect". A security-sensitive change to how the product decides which
   certificates to trust deserves its own lane with its own control test, not the last hour of
   this one.

So this is a **costed, de-risked, ready-to-execute item** rather than a blocker. That is a
better handoff than either "blocked at TLS trust" or a rushed diff.

<!-- append below this line after every measurement -->
