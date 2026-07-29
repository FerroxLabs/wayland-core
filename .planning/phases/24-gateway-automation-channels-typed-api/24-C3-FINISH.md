---
phase: 24-gateway-automation-channels-typed-api
criterion: "24-C3 (reference channels / the inbound matrix)"
lane: 24-c3-finish
branch: lane/24-c3-finish
merge-base: 8bcb052b2aa6b1a9e3f2ed00af935a58c92c1f11
status: complete
grade-24-C3: "STILL NOT MET, and this lane does not claim it. Two of the four untouched clauses are now measured on Linux (health PASS, reconnect/reload PARTIAL — reload registers but does not admit). media and native actions remain untouched for every adapter. A new HIGH was found, proven with a one-variable control, and is NOT fixed."
new-finding: "F24-C3-H5 — `channel reload` registers a new adapter and reports it healthy, but never reloads its inbound access policy, so every message to it is silently denied. Measured, controlled, NOT fixed."
fence-exposure: "zero — 0 Rust files changed, 0 bytes in crates/wcore-cli/src/{lib,main}.rs vs the merge-base SHA"
---

# 24-C3-FINISH — the two reachable clauses, the two determinations, and one new HIGH

**Verdict up front: `24-C3` is NOT MET and I do not claim it.** Five lanes have now
declined to mark it. This one moves two of the criterion's four untouched clauses onto the
board, converts both of the brief's open questions from "blocked" into costed and
executable answers, and finds a new HIGH on the very surface it was re-measuring.

Nothing was merged, no PR opened, nothing tagged, no issue closed, no credential read or
embedded. **Zero Rust files changed**, so the §6 fence exposure is zero by construction.

---

## 1. What I chose, and why

The criterion (`ROADMAP.md:119`) has **eight clauses**: setup/auth, access, routing, media,
native actions, idempotency, reconnect/reload, health. Four prior lanes closed the first
three plus idempotency across five adapters. **Four clauses were untouched on the inbound
path for every adapter.**

That framing decided the order, because it is the only one where the arithmetic is not
close:

| item from the brief | what it buys | chosen rank |
|---|---|---|
| 3 — media / native actions / **reconnect-reload / health** | up to **4 new clauses** | **first** (the two reachable ones) |
| 4 — re-measure `gateway run` | re-validates two overnight fixes; cheap | folded into the same driver |
| 2 — signal / matrix / msteams / imessage | a 6th–9th adapter on clauses **already proven on five** | second, as a survey |
| 1 — email route/bind | a 6th adapter on `routing`, already proven on five | third, as a determination |

Adding a sixth adapter to a clause five adapters already prove is worth strictly less than
proving a clause **zero** adapters prove. So items 1 and 2 were taken as the *determinations*
the brief asked for rather than as builds, and the session's build budget went to clauses.

Items 3 and 4 were folded into one driver deliberately: `health` and `reload` are only
observable **against a running gateway**, which is exactly the surface item 4 wanted
re-measured. Measuring them separately would have started the same gateway twice.

I also front-loaded the two determinations because a measured refusal is itself a
deliverable (§6b-i), and because one of them turned out to collapse the adapter question.

---

## 2. Per-adapter, per-leg results, with counts

All live figures: `hetzner-dsm`, Linux, `/root/wayland-24-c3-finish`,
`wayland-core 0.12.25` built from lane HEAD, **`gateway run`** (not `--json-stream`).

### 2a. The clause matrix — three runs

| run | mode | verdict | legs | arrivals | turns | arrivals bytes | turns bytes | instrument_fault |
|---|---|---|---|---|---|---|---|---|
| 1 | reload | *(discarded — my own instrument, see §5)* | — | 0 | 0 | — | — | route was wrong |
| **3** | **reload** | **FAIL** rc=1 | **7/8** | 2 | 2 | 536 | 572 | **false** |
| **ctl** | **`--all-at-start`** | **PASS** rc=0 | **5/5** | 2 | 2 | 540 | 578 | **false** |

Byte counts are recorded because an empty journal and an absent journal both read as
"0 records" if only parsed records are counted.

### 2b. Per leg

| clause | leg | run 3 (reload) | control (startup) | positive control |
|---|---|---|---|---|
| health | `refuses-with-no-gateway` | **PASS** rc=1 | PASS | n/a |
| health | `reports-running-gateway` | **PASS** | **PASS** | 2 / 3 adapters |
| gateway | `webhook-host-bound` | **PASS** | PASS | n/a |
| gateway | `inbound-arrives-post-fixes` | **PASS** | **PASS** `tier=exact` 3000ms | arrivals=1 |
| reconnect-reload | `new-config-not-picked-up-without-reload` | **PASS** registered=2 | *(skipped)* | n/a |
| reconnect-reload | `reload-registers-the-new-adapter` | **PASS** 2→3, rc=0 | *(skipped)* | registered=3 |
| reconnect-reload | **`reloaded-adapter-actually-carries-inbound`** | **FAIL** | *(skipped)* | **arrivals delta = 0** |
| reconnect-reload | `unchanged-adapter-survives-the-reload` | **PASS** `tier=exact` 1→2 | *(skipped)* | arrivals delta = 1 |
| lifecycle-control | `same-config-admitted-when-present-at-startup` | *(n/a)* | **PASS** `tier=exact` 1→2 | arrivals delta = 1 |

**Every zero above is paired with a live positive control inside the pass condition.** The
brief's universal-denial trap is closed structurally, not by inspection: a leg whose
assertion holds while its control is zero grades FAIL, and the whole run is forced to FAIL
if total arrivals are zero. Both rules are asserted in the self-test (§5).

### 2c. Clause status after this lane

| clause | before | after | where |
|---|---|---|---|
| setup/auth | PROVEN ×5 | unchanged | prior lanes |
| access | PROVEN ×5 | unchanged | prior lanes |
| routing | PROVEN ×5 | unchanged | prior lanes |
| idempotency | PROVEN ×5 | unchanged | prior lanes |
| **health** | **UNTOUCHED** | **PROVEN on Linux** | both runs, with a refusal control |
| **reconnect/reload** | **UNTOUCHED** | **PARTIAL — reload registers, does NOT admit** | run 3, F24-C3-H5 |
| media | UNTOUCHED | **UNTOUCHED** | — |
| native actions | UNTOUCHED | **UNTOUCHED** | — |

`health` is genuinely closed on Linux and the pass is not a count: `configured` and
`registered` are counted from different places by design (F24-D-H2), every per-adapter row
was asserted present and `healthy`, the `reason`-mandatory contract from `health.rs` was
asserted to hold, **and** the surface was proven able to refuse — with no gateway running it
exits non-zero and says so rather than printing a comfortable empty list.

`reconnect/reload` is explicitly **PARTIAL, not PASS.** The reload half registers correctly
and the unchanged adapters survive; the newly-added adapter cannot receive. Grading that
clause PASS on `registered=3` is precisely the false green this criterion has produced
three times, and it is what §4 is about.

---

## 3. NEW HIGH — F24-C3-H5: `channel reload` registers an adapter without its access policy

**One sentence: an operator adds a channel and runs the documented `channel reload`; the
gateway registers it, `channel health` reports it `healthy`, its webhook endpoint answers
`HTTP 200` — and every message to it is silently denied, with no error, no retry and no log
an operator would look at.**

### The measurement

Run 3, from the gateway's own log:

```
4:  [gateway] inbound: subscriber spawned, webhook host listening bind=127.0.0.1:18899 policies=2
12: INFO channel auto-registered channel=f24finthree platform=slack
14: [gateway] channel reload: added=["f24finthree"] replaced=["f24finone","f24fintwo"] removed=[] unchanged=[]
15: INFO inbound denied channel=f24finthree reason=sender not in dm allowlist
```

`policies=2` is the count at startup. The reload added a third adapter and **no policy
republish follows**. Health, taken from a second process at the same moment, read
`configured=3 registered=3 registration_error=none`.

### The control that makes this a product finding rather than a fabricated one

Two hypotheses fit that log identically, and a lane on this program once traced a dedupe
FAIL to its own 90 s replay against a 60 s TTL — reporting it would have been a fabricated
HIGH against working code. So:

- **H1 (product)** reload re-registers adapters but never reloads the inbound policy map.
- **H2 (fixture)** the third config is simply wrong under any lifecycle.

`--all-at-start` changes **exactly one variable**: the third channel is on disk *before* the
gateway starts, so it arrives through the startup path instead of the reload path. Same
config generator (shared function, not re-derived), same minted secrets, same sender, same
token shape, same submit call, same binary, same fixtures, same host.

```
run 3   (reload)  f24finthree → inbound denied, 0 turns, 0 arrivals   FAIL
control (startup) f24finthree → tier=exact, arrivals 1→2, 1 turn      PASS
```

**H2 is excluded. The defect is the reload path.** The control also carries its own control —
a known-good channel in the same process — so a total failure would have graded UNREADABLE
rather than masquerading as a denial of the third.

### Root cause, in source

- `gateway.rs:1101-1157` — the reload block rebuilds the **adapter** set into the shared
  manager. It never touches `inbound_host`.
- `gateway.rs:888-909` — `channel_inbound_host::spawn` is called **once**, at startup.
- `channel_inbound_host.rs:151-152` — `load_channel_policy_configs()` is read there, once;
  `policies_loaded` is fixed for the life of the process.
- `channel_inbound.rs:214` — the map is **moved into the spawned task** (`let policies =
  self.policies;`), a plain owned `HashMap` behind no shared handle.
- `channel_inbound.rs:249-252` — `policies.get(name).cloned().unwrap_or_default()`.
- `access.rs:109-117` — `InboundPolicy::default()` is **fail-closed**: `dm: Allowlist` with
  an EMPTY allowlist, which permits nothing.

So the new channel is absent from a map captured before it existed, falls through to the
fail-closed default, and is denied.

### Severity, argued rather than asserted

**It fails CLOSED, and that part is correct.** The fail-closed default is a deliberate,
well-documented security posture and I am not arguing against it. This is **not** a security
hole — an unconfigured channel denying everything is the right behaviour.

The defect is the **honesty gap around it**: three separate surfaces simultaneously tell the
operator the channel is working. `channel health` says `healthy`. The registration count says
`3`. The webhook endpoint returns `200`. Nothing anywhere reports that the channel cannot
receive. Silent inbound message loss on the persistent runtime Phase 24 installs, on a
documented operator workflow, is the exact shape F24-C3-H4 earned **HIGH** for, and the
falsely reassuring health report makes this one harder to notice than H4 was.

**Graded HIGH.**

### It is NOT fixed, and that is a deliberate call

Severity policy says a HIGH must be fixed or disproved. I proved it real and did not fix it.
Stated plainly rather than dressed up:

The repair is not one-line, and it has **two facets**, only one of which is obvious:

1. the **access policy** map (`channel_inbound.rs:130`) must become a shared, swappable
   handle (`Arc<RwLock<..>>` or equivalent), exposed through `InboundHost`, and refreshed
   from the gateway's reload block;
2. the **tool postures** (`channel_inbound_host.rs:154-171`) are moved into the
   `ChannelTurnDispatcher` by the same code path and go stale identically. A newly-added
   channel would get no posture either.

Fixing only (1) is a **partial repair that would pass a naive re-run** — the reload leg would
go green while the new channel silently ran under a default posture. "The repair was PARTIAL,
and the live run caught it" is a recorded lesson from this very criterion, one lane ago.

That is more than the remaining budget of this session, and it changes the concurrency shape
of the inbound dispatch path. The precedent this program has already validated is the right
one: 24-C3-H2 measured H4 and deliberately did not fix it blind at the end of a lane; H4 then
fixed it properly with a full mutation proof. **Half-building a fix to inbound dispatch is how
a fix becomes the next lane's defect.**

What the next lane inherits is not a hunch: a reproduction, a one-variable control, the six
exact call sites, the two-facet scope, and a driver that already reddens on it. **Estimated
~1 session**, including the mutation proof.

---

## 4. The two determinations the brief asked for

### 4a. Email `route`/`bind` — REACHABLE, with a bounded Rust change. Not from config.

I confirmed the two facts I was told not to re-derive, and did not re-derive them:
`SSL_CERT_FILE` is a Linux/OpenSSL **IMAP-only** seam (`imap.rs:194`), and SMTP resolves to
**`webpki-roots 1.0.7`** with no `rustls-native-certs` and no `rustls-platform-verifier`
(`Cargo.lock:4231-4255`) — compiled-in, reads no file and no env var. I verified the actual
branch taken, `tls.rs:505-510`.

**The part that had not been established:** `TlsParametersBuilder::add_root_certificate`
(`tls.rs:248`) is public and available with this workspace's resolved features, and
`tls.rs:518-526` applies extra roots **unconditionally, after and independent of the
`cert_store` match**. An extra trust anchor can be added *on top of* webpki-roots without
removing them and **without disabling verification**.

The adapter cannot use it because `LettreSender::new` (`smtp.rs:71-84`) passes **no TLS
parameter at all**. That is a seam that does not exist yet — categorically different from
Discord's, which existed and was merely unreachable.

**Cost:** one `#[serde(default)] Option<PathBuf>` on `EmailConfig`, ~15 lines in `smtp.rs`,
plus the control test that a config naming no cert path reaches production SMTP unchanged.
**~1 session**, mirroring `TelegramConfig::api_base_url` exactly.

**The refused shortcut, recorded because it is the obvious one:** the same builder exposes
`dangerous_accept_invalid_certs` (`tls.rs:313`). One line, instant green. It must be refused —
`add_root_certificate` adds an anchor the operator chose; `dangerous_accept_invalid_certs`
removes verification for everyone, in production, permanently. That is shipping a real
security regression to make a test pass.

I did **not** build it: it would add a sixth adapter to a clause five already prove, and a
change to how the product decides which certificates to trust deserves its own lane.

### 4b. The never-driven adapters — two of four need NO Rust at all

Read from the **shipped** construction path (`registry::make_*` → the adapter's `new()`), not
from `#[doc(hidden)]` test constructors — the distinction Discord proved is the whole game.

| adapter | inbound transport | seam reachable from a config file? | Rust needed | cost |
|---|---|---|---|---|
| **matrix** | HTTP long-poll `/sync` | **YES — `homeserver_url`** | **ZERO** | fixture only, ~1 session |
| **signal** | **stdio JSON-RPC subprocess** | **YES — `signal_cli_path`** | **ZERO** | **cheapest in the phase** |
| msteams | webhook + REST | PARTIAL — `service_url` yes, `token_url` **no** | 1–2 config fields | ~2 sessions |
| imessage | SQLite poll + AppleScript | env-only (`$HOME`), send path none | — | **blocked on platform** |

- **matrix** — `MatrixConfig.homeserver_url` (`config.rs:9`) is **required**, has no
  `#[serde(default)]` and no production constant. `new()` consumes it (`lib.rs:61-62`);
  `make_matrix` calls `new()` (`registry:179`). Stronger than telegram's seam: there is **no
  production default to preserve, so there is no control test to write**. Egress is not a
  blocker — same `EgressClient` as telegram/slack/discord, all already driven against
  loopback.
- **signal** — `SignalConfig.signal_cli_path` (`config.rs:18`) → `RealLauncher::launch`
  `Command::new(cli_path).arg("-a").arg(account).arg("jsonRpc")` (`subprocess.rs:53-62`).
  The fixture is **an executable that speaks JSON-RPC on stdio**: no HTTP, no TLS, no port,
  no certificate.
  **This breaks the framing I was given.** The brief said the telegram and Discord precedents
  are both config-level *base-URL* seams. Signal's is a config-level **subprocess-path** seam.
  Anyone grepping the four for `*_base_url` finds nothing in signal and concludes it has no
  seam — when it has the cheapest one in the entire phase.
- **msteams** — exactly Discord's situation. `with_token_url` exists but is `#[doc(hidden)]`
  (`lib.rs:82`) and `new()` hardcodes `BF_TOKEN_URL` (`token.rs:12-13`). It cannot even
  *start* against a fixture, because `start()` must first acquire an AAD token from Microsoft.
- **imessage** — **not blocked on a seam, blocked on the platform.** The crate is
  `#[cfg(target_os = "macos")]` throughout, so it cannot be built on hetzner; the standing
  no-cargo-on-the-Mac constraint means a macOS leg needs the CI-published darwin artifact; and
  the send path is `osascript` behind TCC Automation consent a headless run cannot grant. Do
  not cost this as a fixture problem.

**Correct order for whoever continues: matrix → signal → msteams → imessage.** The first two
need no seam plan at all, only a fixture. The four were being treated as equidistant and they
are not.

---

## 5. Three instrument faults, all mine, all found by running, all repaired here

§6b-ii: a documented instrument defect is a defect you have agreed to keep. Each carries the
mandatory **three** assertions — known-positive passes, known-negative fails, **and the old
broken instrument would have missed it**.

**All three failed in the direction that blames the product.** That is not a coincidence; it
is what an under-detecting instrument does by default, and it is why every one of them had to
be caught by a control rather than by a red.

1. **Wrong webhook route (run 1).** I posted to an invented
   `/channels/:name/slack/events`. `inbound_webhook.rs:12-15` documents the delivery route as
   `POST /webhooks/:channel`. Every submit returned `404`, arrivals stayed 0, and the driver
   graded it **FAIL — reporting my own wrong URL as product inbound loss.**
   Two repairs, not one: the route, **and** an `accepted`/`httpStatus` return so a non-2xx
   submit raises an instrument fault. "The product never received it" and "the product
   received it and dropped it" were producing the same number and are opposite diagnoses.
2. **Token did not match the fixture's correlation contract (run 2).** The product path was
   **flawless** — submit accepted `200`, one LLM turn whose `user_text` carried my exact
   token, one reply delivered to the sink — and the driver still graded FAIL, because
   `f24-llm-fixture.mjs:88-91` extracts with `/f24c3-[a-z0-9-]+/i` and my token was
   `f24c3fin-…`, with no hyphen after `f24c3`, so it echoed the literal `no-correlation`.
   Repaired by **conforming to the shared fixture** rather than editing it — four other
   drivers depend on it. The durable repair is the assertion, not the token: the self-test
   materialises the driver's token template and checks it against the regex **read back out
   of the fixture source**, keeps the failing run-2 shape executable as the known-negative,
   and asserts the fixture extracts the **whole** token rather than a prefix. Either side
   drifting now reddens.
3. **The standalone control could not run at all**, because the product encrypts
   `credentials.toml` into `credentials.enc` under a per-run minted passphrase that is never
   persisted. Folding the control into the driver as `--all-at-start` is the **stronger** form
   anyway: it *shares* `writeSlackChannel` with the experiment, so the config is identical by
   construction. A standalone script re-derives the config, and any divergence in that
   re-derivation silently becomes the variable under test.

**Self-test `f24-c3-clauses-selftest.mjs`: 20 passed / 0 failed**, on macOS and on Linux at
the same commit. It is proven able to fail: mutating `record()` to drop the positive control
gives **11 passed / 1 failed**, and **only the intended assertion reddened** — so the scan
discriminates rather than blanket-failing. Driver restored and re-verified at 20/20.

It also structurally hard-fails any test that returns a thenable, because an async assertion
*rejects* rather than throws and a sibling self-test in this phase scored a deliberately false
assertion as a pass and exited 0.

**The instrument's most important result is negative and worth stating: `instrument_fault =
false` on both graded runs.** The HIGH in §3 is not one of my faults. I had three, I know what
they look like, and this is not one.

---

## 6. Exact remaining distance to `24-C3` MET

| # | gap | owner / cost |
|---|---|---|
| 1 | **`media`** — untouched on the inbound path, every adapter | a lane; the `ChannelMediaEnricher` is already wired at `channel_inbound_host.rs:184-195`, so this is likely cheaper than it looks |
| 2 | **`native actions`** — untouched, every adapter. Discord's fixture already implements typing + reactions REST, so it is the natural first target | ~1 session |
| 3 | **`reconnect/reload`** — reload half PARTIAL (F24-C3-H5); the **reconnect** half (drop the upstream, does the adapter recover?) is untouched. Discord's fixture already implements `op6 RESUME` with replay | ~1 session + the H5 fix |
| 4 | **F24-C3-H5** — measured, controlled, **NOT fixed** | ~1 session, two facets (§3) |
| 5 | **email `route`/`bind`** — reachable, costed, not built | ~1 session (§4a) |
| 6 | **matrix, signal** — zero-Rust seams, undriven | ~1 session each (§4b) |
| 7 | **msteams** | ~2 sessions (§4b) |
| 8 | **imessage** | blocked on platform, not on a seam (§4b) |
| 9 | **macOS + Windows** — every figure in this criterion, from every lane, is **Linux** | needs a build on each |

**Honest total: roughly 6–8 lane-sessions**, and that is before the two other platforms.

`24-C3` is a **release blocker and it is still open.** It is meaningfully closer than it was —
`health` is genuinely closed on Linux, the polling and WebSocket classes are both driven, two
more adapters turn out to need no product change at all, and the email blocker is now a costed
work item rather than a wall. But two clauses have zero evidence on any adapter, a third is
PARTIAL because of a HIGH found today, and that HIGH is unfixed.

**Marking it MET would be wrong, and it would be the worst error available here.**

---

## 7. What I did NOT do

- **Did not mark `24-C3` MET.**
- **Did not fix F24-C3-H5.** Reasoned in §3, not an oversight.
- **Did not measure `media` or `native actions`** — two of the eight clauses, still zero.
- **Did not measure the `reconnect` half** of reconnect/reload, only `reload`.
- **Did not drive matrix, signal, msteams or imessage.** I established their cost only.
- **Did not build the email cert-source knob**, and specifically did **not** reach for
  `dangerous_accept_invalid_certs`.
- **Did not change a single Rust file.** `git diff $BASE --name-only -- '*.rs'` → **0**.
- **Did not touch the §6 fence.** `git diff $BASE -- crates/wcore-cli/src/{lib,main}.rs` →
  empty, against the captured merge-base **SHA** `8bcb052b`, never the branch name.
- **Did not edit `.github/workflows/ci.yml` or `.planning/BACKLOG.md`**, per my lane's
  boundaries.
- **Did not run `wcore-contract generate`**, merge, open a PR, tag, or close an issue.
- **Did not use, read or require any vendor credential.** Every secret in every run was minted
  at run time and died with it.
- **Did not run a full-workspace build or test.** `cargo build --release -p wcore-cli` only,
  per the disk/contention rule.
- **Did not measure anything on macOS or Windows.**
- **Did not modify any shared fixture** (`f24-sink.mjs`, `f24-llm-fixture.mjs`,
  `f24-correlate.mjs`) — four other drivers depend on them; I conformed to their contracts
  instead.

## 8. For the orchestrator to serialize

**Nothing.** Zero Rust files, zero fence bytes, no protocol seam, no contract fixture, no
dependency change. Two new files, both additive and both mine:
`scripts/f24-c3-clauses.mjs` and `scripts/f24-c3-clauses-selftest.mjs`, plus evidence under
`.planning/phases/24-gateway-automation-channels-typed-api/24-C3-FINISH-evidence/`.

`f24-c3-clauses.mjs` exit codes: **0 GREEN, 1 RED, 2 USAGE, 3 INCOMPLETE (instrument fault)**.
A gate treating non-zero as uniform failure still works; one distinguishing a harness fault
from a product regression must read the code.

## 9. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-C3-FINISH-evidence/`

| path | what it holds |
|---|---|
| `24-C3-FINISH-NOTES.md` | the running log, committed at T+10 min and re-committed after every measurement |
| `run3-reload-FAIL/` | `result.json` (8 legs), `gateway.log` (the `policies=2` / `inbound denied` sequence), `journals.txt` |
| `control-all-at-start-PASS/` | `result.json` (5 legs), `gateway.log`, `journals.txt` — the one-variable control that excludes H2 |
