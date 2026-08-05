# 24-03 SURFACE CONTRACT — channel framework and typed client

Lane `lane/24d`. Base `b303a366`. Linux host `hetzner-dsm`, worktree `/root/wayland-24d`.

This document holds the recorded matrix, the live transcripts, and the mutation
measurements. It records what was measured. Where an element was not exercised,
it says so in the same table rather than in a footnote.

---

## 1. The channel framework matrix

`crates/wcore-channels/tests/framework_matrix.rs` — 17 cases. Element by
element, with the state each case had to carry in order to be able to fail.

| # | Contract element | Where it lives | Case | Proven at |
|---|---|---|---|---|
| 1 | Setup/auth probe, three questions, no message sent | `probe.rs` | default is a NAMED `Unsupported`, not a green | unit + live |
| 2 | Probe distinguishes incomplete / unauthenticated / unreachable | `probe.rs`, discord, email | three distinct outcomes, three operator actions | unit + live |
| 3 | Probe never emits the credential (T-24-03-06) | `probe.rs` | canary with positive control | unit |
| 4 | Probe listing omits nobody | `manager.rs` | an unprobeable adapter still appears, as `Unsupported` | unit |
| 5 | Pairing/access reuses the existing decision path | `dispatch/access.rs` | **not re-implemented** — consumed unchanged | pre-existing |
| 6 | Binding: unbound conversation takes a DECLARED default | `binding.rs` | `BindingTable::new` requires it; no constructor omits it | unit |
| 7 | Binding: thread > conversation > space specificity | `binding.rs` | each level, plus a sibling thread unaffected | unit |
| 8 | Binding keys cannot be impersonated across the separator | `binding.rs` | a conversation named `general/t42` does NOT inherit thread `t42` | unit + **M2** |
| 9 | Media normalisation onto existing kinds | `media.rs` | MIME families, extension fallback, content-type wins | unit |
| 10 | Media: declared bound, per adapter | `media.rs`, trait `media_bounds` | discord 25 MiB / email 10 MiB, enforced not hardcoded | unit |
| 11 | Media: EXPLICIT degradation, never a silent drop | `media.rs` | oversize and over-count both retain the attachment WITH a reason | unit |
| 12 | Edit / delete / reaction: named unsupported, never a silent Ok | `lib.rs`, `error.rs` | new `ChannelError::Unsupported`, distinct from `Rejected` | unit |
| 13 | …and an implementing adapter really performs them | test adapter | counters assert the call landed | unit |
| 14 | Health: registered-but-unpolled is `Unknown`, not `Healthy` | `health.rs` | unit + **M3** | unit + live |
| 15 | Health: every non-healthy state carries a reason | `health.rs` | invariant asserted, and proved able to fail | unit + live |
| 16 | Health: auth failure distinct from transport failure | `manager.rs` | adapter-published `AuthError` → `Unauthenticated` | unit |
| 17 | Reconnect observable, with a flap counter | `manager.rs` | Degraded → Healthy with `reconnects >= 1` | unit |
| 18 | Reload keeps an unchanged adapter's RUNNING INSTANCE | `manager.rs` | receipt instance tag, a fact reload does not author | unit |
| 19 | Reload replaces a changed adapter | `manager.rs` | rotated fingerprint → new instance answers | unit |
| 20 | Reload treats "cannot tell" as CHANGED | `manager.rs` | unit + **M4** | unit |
| 21 | Reload adds and removes | `manager.rs` | health entry removed with the adapter | unit |
| 22 | Outbound idempotency | 24-C | **landed by lane 24c**, consumed here unchanged | 24-C |

**Element 5 and 22 were not re-implemented.** The access-decision path and the
outbound-idempotency ledger already existed; this plan's own rules say to
consume them, and re-doing either would have been the second implementation
AGENTS.md forbids.

### Reference adapters — a deliberately DIFFERENT pair of shapes

The plan names one persistent-connection adapter and one polling adapter, and
the point is that they exercise different halves of the contract.

| | discord (persistent) | email (polling) |
|---|---|---|
| probe cost | one HTTP round trip | four credential-handle lookups + a real IMAP session |
| what authenticates | one bot token | IMAP account, across two protocols' config |
| identity returned | bot user id from `/users/@me` | the IMAP account |
| proven against | local `mockito` endpoint, no vendor credential | live, against a dead local port and an empty store |
| stated gap | — | SMTP credentials are checked for PRESENCE only; a present-but-wrong SMTP password passes the probe and fails at first send |

Discord's probe carries five cases (ok-with-identity, 401 → unauthenticated,
refused connection → unreachable, absent credential → incomplete, absent handle
→ incomplete) plus a canary case, all against `mockito`. **No Discord token and
no vendor network reach is involved in any of them.**

---

## 2. The typed client — four contracts

`crates/wcore-acp/src/{roles,idempotency,cursor,negotiate}.rs`. 125 unit tests
in the crate after the addition.

| Contract | Property that had to be able to fail | Proven |
|---|---|---|
| Roles | a refusal is `Forbidden`, never `Auth`; 403 not 401 | unit |
| Roles | no recognised role ⇒ denied EVERYTHING, including reads | unit |
| Roles | an unclassified method requires Admin, so an omission is loud | unit + **M10** |
| Roles | the refusal names required and held role, and NOT the principal | unit |
| Idempotency | one identity twice ⇒ one EFFECT (counted) and two identical receipts | unit |
| Idempotency | a different command under a used identity is a named conflict | unit |
| Idempotency | a full ledger REFUSES rather than evicting a guarantee | unit + **M9** |
| Idempotency | a full ledger still replays a KNOWN identity | unit |
| Cursor | missed events, in order, exactly once | unit |
| Cursor | a position AHEAD is refused, not answered with an empty list | unit + **M7** |
| Cursor | an evicted position is refused WITH the oldest servable | unit |
| Cursor | the eviction boundary is exact (off-by-one drops one event per resume) | unit |
| Cursor | a cursor from another STREAM is refused even when in range | unit + **M6** |
| Cursor | retention is bounded; positions never reused | unit |
| Negotiate | under-version refused BY NAME, never silently downgraded | unit |
| Negotiate | comparison is numeric — `"0.10" < "0.9"` as strings, opposite numerically | unit |
| Negotiate | the server version is READ from the handshake constant, not restated | unit |

### The cursor case worth reading

A bare `u64` is not a resumable cursor. After a restart an in-memory log
renumbers from 1, so a client resuming at position 2 is handed positions 3.. of
a **different** stream, believes itself continuous, and has silently missed the
new stream's 1 and 2. Nothing errors. Nothing is duplicated. **The server's own
counts look perfect** — which is the same shape lane 24c measured at the
independent sink, where a ledger reporting `carried=1 (unknown-outcome 1)` sat
beside a destination holding two copies of one message.

`Cursor` therefore carries the stream identity, and the test for it has a
positive control: the same POSITION on the right stream is asserted servable
first, so the refusal is attributable to the identity and to nothing else.

---

## 3. Support bundle — T-24-03-01 (CRITICAL)

Two defences in a deliberate order. **Structural elision first**: config,
credentials store and environment contribute KEY NAMES only and their values
are never read into the bundle. **Exact-secret scrubbing second**, over free
text only, as a backstop that can only ever catch enumerable secrets.

Getting the order backwards — copy values, then scrub — fails open for every
secret the redactor never learned, which in production is most of them.

`key_names` is a lexical scan, not a parse, because a parse must hold values in
a structure a later `Debug` or serialization could emit.

### Deviation: the bundle is a DIRECTORY, not a `.tar.gz`

The plan's canary gate runs `tar xzOf`. `tar` and `flate2` are workspace
dependencies but not `wcore-gateway`'s, and adding either rewrites `Cargo.lock`
— a Phase-24 shared seam. An uncompressed tree is also the stronger posture:
the scan reads real bytes rather than trusting a decompressor, there are no
nested members a shallow scan could miss, and an operator can read exactly what
they are about to send. The equivalent scan (`bundle_files` + a byte-window
search over every file, recursively, text or not) replaces the decompression.

### Live canary, on the home a real gateway had just run in

```
canary file: 42 bytes
seeded:      /tmp/f24d-live/config.toml, /tmp/f24d-live/gateway.log
collect ->   members=["config-keys.txt","environment-keys.txt",
                      "gateway-status.json","channel-health.json","recent-log.txt"]
             known_secrets=2 redactions=1
             absent=["credentials: /tmp/f24d-live/credentials.toml"]

live gate WITHOUT its env vars:
  panicked: F24_LIVE_BUNDLE must be set; this gate FAILS rather than skips
  rc=101                                   <- it fails, it does not skip

live gate WITH them:
  test live_bundle_canary ... ok           rc=0

independent double-check, a raw recursive grep of every bundle byte:
  no occurrence in any bundle file

the elided config member:
  [providers.anthropic]
  api_key  [value elided]

the scrubbed log tail:
  [gateway] auth failed using token [REDACTED]
```

The bundle was produced by a test driver calling the production `collect`, not
by an operator verb. **There is no `gateway support-bundle` CLI verb** — named
gap, §6 of the SUMMARY.

---

## 4. Live journey — the SHIPPED binary

`/root/wayland-24d/target/release/wayland-core`, version 0.12.25, hermetic home
`WAYLAND_HOME=/tmp/f24d-live`, two email channels configured, **no vendor
credential anywhere**.

The scenario is deliberately not clean: the channels are misconfigured on
purpose, because a probe that can only report success is not a probe and a
health surface with nothing wrong reports nothing distinguishable from a broken
one.

```
1. channel list, empty dir      -> "no channels configured in /tmp/f24d-live/channels"   EXIT=0
2. channel list, two seeded     -> broken / deadport, both email, enabled                EXIT=0

3. channel health, NO gateway   -> "no running gateway ... channel health is an
                                    OBSERVATION and nothing has observed anything"       EXIT=1
4. channel reload, NO gateway   -> "nothing would act on a reload request"               EXIT=1

5. channel probe --json         -> both channels `"outcome": "incomplete"`, each naming
                                   FOUR absent credential handles by name:
                                     smtp.user_credential_handle -> "email.broken.smtp_user"
                                     ... (never a value)
                                -> "2 of 2 channels are not ready: broken, deadport"     EXIT=1

6. gateway run --detach         -> pid 2169382; gateway status: Running                  EXIT=0

7. channel health, gateway UP   -> configured: 2   registered: 2
                                   broken (email)   state: Disconnected
                                     reason: start() failed: auth failed: smtp user not
                                             found at credential_handle "email.broken.smtp_user"
                                   deadport (email) state: Disconnected  (same shape)     EXIT=0
   raw published file, written by the gateway and read by a DIFFERENT process:
                                   {"configured":2,"registered":2,"channels":[...]}

8. channel reload, gateway UP   -> "reload requested; gateway pid 2169382 will re-read"   EXIT=0

9. kill -9 2169382              -> stale channel-health.json STILL ON DISK
   channel health                -> refuses again, same message                          EXIT=1
```

**Step 9 is the load-bearing one.** The published file is still there, fully
parseable, describing two adapters. `channel health` reports nothing, because
the process that observed them is gone. A health surface that answered here
would be reporting an observation nobody made.

**Step 5 and step 7 disagree, correctly.** `probe` says `incomplete` because it
asked the credentials store; `health` says `Disconnected` with a reason,
because that is what the running gateway's `start()` actually got. Two
questions, two sources, two answers — which is the point of §1 of
`crates/wcore-cli/src/channel.rs`.

### What the first live run found

Two HIGH defects, both invisible to the suite and to clippy, both fixed and
re-measured. See SUMMARY §5 — F24-D-H1 (the channel subsystem was silently
disabled on any host without an LLM provider key) and F24-D-H2 (`channel
health` rendered a failed registration as "you have no channels", the exact
false zero this lane exists to close, reintroduced by the code closing it).

---

## 5. Mutations — ten, each reddening its named test and restoring byte-identically

Every mutation is preceded by a BASELINE run of the same filter. A mutant that
reddens a test which was already red proves nothing, so `baseline_rc=0` is
recorded beside every result. Restoration is verified by SHA-256, not assumed.

| | Mutation | baseline | mutant | restore |
|---|---|---|---|---|
| M1 | probe default returns a green instead of a named `Unsupported` | 0 | 101 | sha-equal |
| M2 | binding key stops escaping the separator | 0 | 101 | sha-equal |
| M3 | a registered-but-unpolled channel reads `Healthy` | 0 | 101 | sha-equal |
| M4 | reload treats an unfingerprintable adapter as UNCHANGED | 0 | 101 | sha-equal |
| M5 | `process_is_alive` drops the pid-zero guard | 0 | 101 | sha-equal |
| M6 | cursor stops checking the stream identity | 0 | 101 | sha-equal |
| M7 | cursor answers an impossible position with an empty list | 0 | 101 | sha-equal |
| M8 | support bundle keeps config VALUES alongside the names | 0 | 101 | sha-equal |
| M9 | a full idempotency ledger evicts instead of refusing | 0 | 101 | sha-equal |
| M10 | an unclassified method falls through to the LEAST privilege | 0 | 101 | sha-equal |

M2 reddens exactly two cases and leaves three green:

```
test binding::tests::escaping_is_reversible_enough_to_stay_injective ... FAILED
test binding::tests::a_conversation_id_cannot_impersonate_a_thread_of_another_conversation ... FAILED
test result: FAILED. 3 passed; 2 failed
```

### F24-D-P1 — the mutation harness was itself self-passing, and said so

M2 reported `baseline_rc=0 mutant_rc=0` on its first run. The mutation was
correct; the HARNESS was wrong. It ran `--test framework_matrix` with the
filter `cannot_impersonate`, but that test is an INLINE unit test in
`src/binding.rs`, so the filter matched **zero tests** and `cargo test` exited 0
both times.

This is an eleventh entry for the standing self-passing list, and a new shape:
**a test filter that matches nothing reports success.** It was caught only
because the harness recorded a baseline and compared the pair, rather than
asserting on the mutant alone. A harness that only checked `mutant_rc != 0`
would have reported M2 as unproven; one that only checked "the mutation was
applied" would have reported it as proven. The pair is what caught it.

---

## 6. Gate table

| Gate | Command | Result |
|---|---|---|
| Tests, 13 crates | `cargo test --no-fail-fast -p wcore-channels -p wcore-channels-registry -p wcore-acp -p wcore-gateway -p wcore-channel-{discord,email,slack,telegram,matrix,signal,sms,whatsapp,msteams}` | **727 passed, 0 failed, 1 ignored**, rc=0 |
| Clippy, core | `cargo clippy -p wcore-channels -p wcore-channels-registry -p wcore-acp -p wcore-gateway -p wcore-channel-discord -p wcore-channel-email --all-targets -- -D warnings` | rc=0 |
| Clippy, CLI | `cargo clippy -p wcore-cli --all-targets -- -D warnings` | rc=0 |
| fmt (the one Cargo command permitted on the Mac) | `cargo fmt --all -- --check` | rc=0 |
| Seam, vs the MERGE-BASE, paths inlined | `git diff --quiet b303a366 -- Cargo.toml Cargo.lock crates/wcore-config/src/config.rs crates/wcore-protocol` | rc=0 |
| Seam CONTROL | same gate against `crates/wcore-acp/src/lib.rs` | **rc=1** |
| §6 fence | `git diff --stat b303a366 -- crates/wcore-cli/src/{lib,main}.rs` | **18 insertions, 0 deletions** |
| Live canary, no env | `cargo test ... --ignored live_bundle_canary` | **rc=101 — fails, does not skip** |
| Live canary, with env | same, against a bundle from a real gateway home | rc=0, positive control satisfied |

No gate here terminates in a pipe. Every exit status is captured into a
variable on the line after the command, before any filtering.

### F24-D-P2 — a seam gate written against a BRANCH NAME mis-attributes other lanes

The seam gate was first run as `git diff --quiet plan/f20-unified-audit-repair
-- <paths>`. That branch has moved 
since this lane was cut (`b303a366` → `32e2f57d`), so the diff also showed other
lanes' merged work, and the §6 fence check reported **28 deletions this lane
never made** — `pub mod backup;` and `pub mod node;`, both ADDED upstream after
the branch point.

The gate must name the MERGE-BASE SHA, not the moving branch. Against
`b303a366` the same fence check reports 18 insertions and 0 deletions, which is
the truth. Any lane in this program running a diff against the branch name is
reading a number that grows as other lanes land.
