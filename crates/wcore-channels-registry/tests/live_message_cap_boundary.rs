//! LIVE message-cap boundary probe — the committed artifact
//! [wayland#934](https://github.com/FerroxLabs/wayland/issues/934) item 2 asks for.
//!
//! # The claim this file settles, and the one it must not be cited for
//!
//! Every capped adapter declares a single-message limit through
//! [`Channel::max_message_len`]. `ChannelManager::send_to_keyed` chunks on that
//! number and — per `docs/delivery-semantics.md` §4.1 — **drops the idempotency
//! key** above it. The cap is therefore load-bearing for the only exactly-once
//! guarantee the product has, and for HIGH-6 (a reply the platform rejects and
//! nobody re-sends).
//!
//! Everything that checked those numbers before this file compared two numbers
//! we wrote to each other:
//!
//! * the per-adapter unit tests (`a_body_over_the_cap_splits_into_pieces_…`)
//!   drive `ChannelManager::chunks_for` at the cap and one char over. That
//!   tests the CHUNKER against the constant, and passes whatever the constant
//!   says;
//! * `delivery_semantics_declaration.rs` compares the constant against
//!   `docs/delivery-semantics.md`. That is drift detection, and both numbers
//!   are ours.
//!
//! Neither can fail when the number is wrong **about the platform**, which is
//! the only way a cap can be wrong that costs a customer a message. Slack
//! proved it: `slack.cap` was 39,000 for months — 9.75x the real boundary —
//! and every test in this repository stayed green, because a 39,000-char body
//! left as ten Slack messages while `chunks_for(..).len() <= 1` reported a
//! single keyed delivery.
//!
//! This file is where the platform gets a vote.
//!
//! # Three shapes, because one shape could not decide two of the cells
//!
//! The cell types live in [`cells`], and its module documentation is the
//! argument. In short: a two-point probe at `cap` and `cap + 1` decides a
//! CHARACTER cap and cannot decide a BYTE-BUDGET one, because in ASCII both
//! arms land a quarter of the way into the budget and are accepted. Matrix and
//! MS Teams sat in that shape carrying a credential as their blocker, and a
//! credential would not have helped (wayland#934 c7). They are now
//! [`Boundary::Derived`], decided hermetically by [`derivation_faults`] and, at
//! a real destination, by a single SATURATING arm rather than two ASCII points.
//!
//! # What a boundary is here, and why it is not always `cap + 1`
//!
//! For a character cap the probe drives two sends at a real destination: one of
//! exactly `Cell::probe_at` characters, which must arrive as ONE message, and
//! one of `+ 1`, which must do whatever [`Above`] records for that platform.
//!
//! The second outcome is **not always a rejection**, and writing the probe as
//! "expect an error at `cap + 1`" would have been a probe that fails against a
//! correct product:
//!
//! * **Discord** refuses `2,001` with a catchable HTTP 400 `50035 Invalid Form
//!   Body`. That one is assertable.
//! * **Slack** accepts `4,041` and *silently splits it into 4,000-char
//!   messages*. There is no error to catch. The declared cap of 4,000 is below
//!   the 4,040-char boundary on purpose, and the failure mode above it is
//!   silent data reshaping rather than a refusal.
//!
//! So the assertion that carries the weight is the SAFETY property, not an
//! equality: **the cap we ship must never exceed the boundary the platform
//! actually honours.** A cap at or below the boundary chunks a little earlier
//! than it strictly must (costing an idempotency key, §4.1); a cap above it
//! reinstates HIGH-6. Those two are not symmetrical, and this file only refuses
//! the expensive direction.
//!
//! # Why this constructs the adapter instead of using `ChannelManager`
//!
//! `ChannelManager::send_to_keyed` is the production send, and it **chunks**.
//! Driving `accepts_up_to + 1` through it would measure our own chunker for a
//! second time rather than the platform, which is the exact tautology this file
//! exists to break. So [`production_adapter`] replicates
//! `auto_register_from_dir`'s construction chain verbatim —
//! `parse_channel_config` → [`channel_factory_for`] → `factory(..)` → `start()`
//! — and keeps the `Box<dyn Channel>` so the over-boundary body reaches the
//! platform in one piece. Everything about how the adapter is built, configured
//! and authenticated is the production path; only the chunker is stepped
//! around, deliberately and only here.
//!
//! # The guard walks what the registry can BUILD, not a list of platforms
//!
//! [`shipped_caps`] enumerates
//! [`wcore_channels_registry::constructible_selectors`] rather than the
//! platform strings in the declaration. That is wayland-core#360 c2, and it is
//! not a tidy-up: the WhatsApp bridge is an eighth and ninth `max_message_len`
//! reached through the `whatsapp` platform string plus a `backend` key, so a
//! platform-keyed guard could not see it however carefully anyone read the
//! list. Two cells at the bottom of [`CELLS`] exist because the guard can now
//! demand them.
//!
//! # A probe that cannot run must not read as a pass
//!
//! Four of the nine cells have ever been driven, and none of the four settled
//! its unit. The rest name what they need, and none of them says "later".
//!
//! * The live cells are `#[ignore]`d and every one of them **panics naming the
//!   missing variable** when invoked with `--ignored` and an incomplete
//!   environment. An env-gated `return` is the shape that printed `5 passed`
//!   for zero work in `live_integrity.rs`.
//! * [`unprobed_caps_are_visibly_unrun`] is **not** `#[ignore]`d. It prints the
//!   census of never-driven cells on every ordinary `cargo test`, so "nobody
//!   has ever measured this" stays visible in CI output instead of living in a
//!   doc comment somebody has to remember.
//! * [`a_shipped_cap_never_exceeds_its_measured_boundary`],
//!   [`a_derived_cap_is_exactly_what_its_budget_admits`],
//!   [`a_settled_unit_verdict_is_enforced_against_the_shipped_cap`] and
//!   [`every_live_verdict_in_the_declaration_has_a_probe_cell_here`] are not
//!   `#[ignore]`d either. They are the part of the measurement that survives
//!   the run.
//!
//! # Running one
//!
//! ```text
//! WL_LIVE_CAP_DISCORD_HOME=/path/to/home \
//! WL_LIVE_CAP_DISCORD_CHANNEL=my-discord \
//! WL_LIVE_CAP_DISCORD_TO=<snowflake> \
//!   cargo test -p wcore-channels-registry --test live_message_cap_boundary \
//!   -- --ignored --nocapture live_boundary_at_real_discord
//! ```
//!
//! `HOME` is a directory holding `channels/<CHANNEL>.toml` and
//! `credentials.toml`, exactly as `gateway run` reads them. `TO` is the
//! conversation the probe posts into — it posts a message the size of the
//! platform's limit and then deletes it where the adapter supports deletion, so
//! point it at a channel you own.
//!
//! Add `WL_LIVE_CAP_<PLATFORM>_ASTRAL=1` to drive the unit arm instead of the
//! ASCII one: the same two points in astral-plane characters, which is what
//! settles characters-versus-UTF-16-code-units (wayland#934 c8).

#[path = "live_message_cap_boundary/cells.rs"]
mod cells;
#[path = "live_message_cap_boundary/driver.rs"]
mod driver;

use std::sync::Arc;

use wcore_channels_registry::{ChannelSelector, channel_factory_for, constructible_selectors};
use wcore_config::credentials::CredentialsStore;

use cells::{
    Above, Boundary, ByteBudget, CELLS, CapUnit, Cell, Saturating, cell, derivation_faults,
    unit_safety_faults,
};
use driver::{drive_cell, required};

/// The declaration, read from source at test time. `include_str!` rather than a
/// runtime path read, for the same reason
/// `delivery_semantics_declaration.rs` does it: a wrong path is then a compile
/// error rather than a silent zero-row parse that makes every assertion vacuous.
const DECLARATION: &str = include_str!("../../../docs/delivery-semantics.md");

// ---------------------------------------------------------------------------
// The shipped constants, read the way the shipped binary reads them
// ---------------------------------------------------------------------------

/// In-memory credentials for the hermetic tests. No adapter reads a credential
/// during construction, but the factory signature requires a store and reaching
/// for the real keyring in a test would touch the developer's own secrets.
struct MemStore;

impl CredentialsStore for MemStore {
    fn get(
        &self,
        _key: &str,
    ) -> Result<Option<String>, wcore_config::credentials::CredentialsError> {
        Ok(None)
    }
    fn put(
        &self,
        _key: &str,
        _value: &str,
    ) -> Result<(), wcore_config::credentials::CredentialsError> {
        Ok(())
    }
    fn delete(&self, _key: &str) -> Result<(), wcore_config::credentials::CredentialsError> {
        Ok(())
    }
}

/// Hermetic per-selector config, sufficient to construct the implementation.
/// Mirrors `delivery_semantics_declaration.rs`'s fixtures; no adapter contacts a
/// network during construction.
fn fixture_options(selector: &ChannelSelector) -> toml::Table {
    let body: &str = match selector.key.as_str() {
        "slack" => {
            "workspace_name = \"fixture\"\n\
             credential_handle_bot_token = \"fixture.slack.bot_token\"\n\
             credential_handle_signing_secret = \"fixture.slack.signing_secret\"\n"
        }
        "telegram" => "credential_handle = \"fixture.telegram.bot_token\"\n",
        "discord" => "credential_handle = \"fixture.discord.bot_token\"\n",
        "email" => {
            "from_address = \"bot@fixture.invalid\"\n\
             [smtp]\n\
             host = \"smtp.fixture.invalid\"\n\
             user_credential_handle = \"fixture.email.smtp_user\"\n\
             password_credential_handle = \"fixture.email.smtp_pass\"\n"
        }
        "signal" => "account = \"+15550000000\"\n",
        "imessage" => "",
        "sms" => {
            "from_number = \"+15550000000\"\n\
             credential_handle_account_sid = \"fixture.sms.account_sid\"\n\
             credential_handle_auth_token = \"fixture.sms.auth_token\"\n"
        }
        "whatsapp" => {
            "workspace_name = \"fixture\"\n\
             phone_number_id = \"10000000000\"\n\
             credential_handle_access_token = \"fixture.whatsapp.access_token\"\n\
             credential_handle_app_secret = \"fixture.whatsapp.app_secret\"\n"
        }
        // Both bridged selectors take the same config; `backend` is added by
        // the selector itself, which is the point of `apply`.
        "whatsapp+baileys" | "whatsapp+whatsapp-web" => {
            "bridge_path = \"/definitely/not/here/bridge.js\"\n"
        }
        "matrix" => {
            "homeserver_url = \"https://matrix.fixture.invalid\"\n\
             credential_handle_access_token = \"fixture.matrix.access_token\"\n\
             user_id = \"@fixture-bot:fixture.invalid\"\n"
        }
        "msteams" => {
            "credential_handle_app_id = \"fixture.msteams.app_id\"\n\
             credential_handle_app_password = \"fixture.msteams.app_password\"\n"
        }
        other => panic!(
            "no fixture config for selector {other:?}; a new implementation needs one here as \
             well as a probe cell"
        ),
    };
    let mut table: toml::Table = toml::from_str(body).expect("fixture config must parse");
    selector.apply(&mut table);
    table
}

/// Build every implementation the registry can construct and return the ones
/// that declare a finite cap, as the SHIPPED BINARY builds them (through
/// `channel_factory_for`, the factory `auto_register_from_dir` uses).
///
/// Keyed by SELECTOR, not by platform: see the module docs. A `whatsapp`
/// platform string reaches three different implementations and two of them are
/// the bridge.
fn shipped_caps() -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for selector in constructible_selectors() {
        let Some(factory) = channel_factory_for(selector.platform) else {
            continue;
        };
        let ch = factory(
            format!("fixture-{}", selector.key),
            &fixture_options(&selector),
            Arc::new(MemStore),
        )
        .unwrap_or_else(|e| {
            panic!(
                "could not construct {:?} from its fixture: {e}",
                selector.key
            )
        });
        if let Some(cap) = ch.max_message_len() {
            out.push((selector.key, cap));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The declaration's machine-readable block
// ---------------------------------------------------------------------------

const BEGIN: &str = "<!-- DELIVERY-SEMANTICS-MACHINE-READABLE";
const END: &str = "-->";

fn machine_block() -> &'static str {
    let start = DECLARATION
        .find(BEGIN)
        .expect("the machine-readable block must exist; a zero-row parse is a vacuous test")
        + BEGIN.len();
    let rest = &DECLARATION[start..];
    let end = rest
        .find(END)
        .expect("the machine-readable block must be closed");
    &rest[..end]
}

/// Selector keys whose `<key>.cap_measured` verdict is `live`.
fn keys_claiming_live() -> Vec<String> {
    machine_block()
        .lines()
        .filter_map(|l| l.split_once('='))
        .filter_map(|(k, v)| {
            k.trim()
                .strip_suffix(".cap_measured")
                .filter(|_| v.trim() == "live")
                .map(str::to_string)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Hermetic tests — these run on every ordinary `cargo test`
// ---------------------------------------------------------------------------

/// **The assertion the tautology could never make.**
///
/// For every implementation whose boundary has actually been driven, the cap the
/// SHIPPED adapter returns must be at or below the largest body that platform
/// accepted. The right-hand side is not a number this repository returns from
/// anywhere — it is a recorded observation of a third party — so unlike
/// `assert_eq!(ch.max_message_len(), Some(4_000))` this cannot be satisfied by
/// editing the constant.
///
/// Direction matters and the inequality is deliberate. A cap BELOW the boundary
/// chunks a body that need not have been chunked, which costs an idempotency
/// key (`docs/delivery-semantics.md` §4.1) — visible, bounded, and the safe
/// direction. A cap ABOVE it hands the platform a body it will refuse or
/// silently reshape, which is HIGH-6: the reply is lost and nothing re-sends
/// it. Only the second is refused here.
#[test]
fn a_shipped_cap_never_exceeds_its_measured_boundary() {
    let caps = shipped_caps();
    assert!(
        !caps.is_empty(),
        "constructed zero capped adapters, which would make this test vacuous"
    );
    let mut checked = 0usize;
    for (key, cap) in &caps {
        let Boundary::Measured {
            accepts_up_to,
            above,
            on,
            ..
        } = cell(key).boundary
        else {
            continue;
        };
        checked += 1;
        assert!(
            *cap <= accepts_up_to,
            "{key}: the shipped cap is {cap} but the platform accepted at most {accepts_up_to} \
             chars as a single message (measured {on}; above it: {above:?}). A cap over the \
             boundary is HIGH-6 — send_to_keyed hands the platform a body it will not deliver \
             whole, and nothing re-sends it."
        );
    }
    assert!(
        checked > 0,
        "no cell in CELLS is Measured, so this test asserted nothing. Either a boundary run was \
         recorded and this is a parsing bug, or the measured cells were deleted — and deleting a \
         measurement is how it stops being visibly present."
    );
}

/// **The check that decides a byte-budget cap, and it needs no credential.**
///
/// wayland#934 c7. The two-point probe cannot settle Matrix or MS Teams because
/// both ASCII arms land inside the accepted region. What CAN be settled without
/// a platform is the arithmetic the derived number claims: `cap` scalars must
/// spend at most the budget at the worst-case encoding, and must be the LARGEST
/// scalar count for which that holds. Both of the two historical mistakes
/// violate it — see [`derivation_faults`] — and neither was visible to a
/// cap-versus-document comparison, because both halves of that comparison were
/// ours.
#[test]
fn a_derived_cap_is_exactly_what_its_budget_admits() {
    let caps = shipped_caps();
    let mut checked = 0usize;
    for (key, cap) in &caps {
        let Boundary::Derived(budget) = cell(key).boundary else {
            continue;
        };
        checked += 1;
        let faults = derivation_faults(*cap, &budget);
        assert!(
            faults.is_empty(),
            "{key}: the shipped cap disagrees with the budget it is derived from:\n  {}",
            faults.join("\n  ")
        );
    }
    assert_eq!(
        checked, 2,
        "expected the two byte-budget cells (matrix, msteams) to be checked here. A shrinking \
         count means a derived cap stopped being derived, or a cell was deleted — and a byte \
         budget with no derivation check is a number nobody can re-derive."
    );
}

/// **Can it fail** — the same checker, over the numbers that actually shipped.
///
/// A derivation check that has only ever seen correct inputs is
/// indistinguishable from one that returns an empty vector. These are not
/// hypothetical inputs: 32,768 is the `matrix.cap` this product shipped until
/// 2026-08-28, and 28,000 is the `msteams.cap` beside it. Both passed every
/// test in the repository at the time.
#[test]
fn the_derivation_checker_rejects_the_two_caps_that_actually_shipped_wrong() {
    let matrix = match cell("matrix").boundary {
        Boundary::Derived(b) => b,
        other => panic!("matrix is no longer a byte-budget cell: {other:?}"),
    };
    let msteams = match cell("msteams").boundary {
        Boundary::Derived(b) => b,
        other => panic!("msteams is no longer a byte-budget cell: {other:?}"),
    };

    // Known-positive first: the shipped numbers are clean, so a red below is
    // caused by the mutation and not by a checker that reports everything.
    assert!(derivation_faults(16_384, &matrix).is_empty());
    assert!(derivation_faults(20_480, &msteams).is_empty());

    let was = derivation_faults(32_768, &matrix);
    assert_eq!(was.len(), 1, "got: {was:?}");
    assert!(
        was[0].contains("131072") && was[0].contains("65536"),
        "the fault must name the bytes a 32,768-scalar body costs and the budget: {was:?}"
    );

    let teams = derivation_faults(28_000, &msteams);
    assert_eq!(teams.len(), 1, "got: {teams:?}");
    assert!(
        teams[0].contains("112000") && teams[0].contains("81920"),
        "got: {teams:?}"
    );

    // And the other direction, which is safe but not derived: a cap far below
    // the budget is reported too, so "derived" keeps meaning derived.
    let low = derivation_faults(1_000, &matrix);
    assert_eq!(low.len(), 1, "got: {low:?}");
    assert!(low[0].contains("16384"), "got: {low:?}");
}

/// A byte-budget cell must never be recorded as though the two-point probe
/// decided it.
///
/// This is the other half of wayland#934 c7. Matrix and MS Teams were
/// `NotMeasured` with a CREDENTIAL named as the blocker, which reads as "run it
/// when the token arrives" — and a token would have bought two accepted arms
/// and no verdict. The shape has to be stated in the cell, and the cell has to
/// say what the ASCII arms do, or the next person re-derives the same dead end.
#[test]
fn a_byte_budget_cell_states_why_the_two_point_probe_cannot_decide_it() {
    let mut seen = 0usize;
    for c in CELLS {
        let Boundary::Derived(b) = c.boundary else {
            continue;
        };
        seen += 1;
        assert!(
            matches!(b.ascii_two_point, Above::AcceptedNormally(_)),
            "{}: a byte-budget cap whose ASCII arm at cap + 1 is anything but AcceptedNormally is \
             not a byte-budget cap — it has a character boundary the two-point probe can reach, \
             so record it as Measured or NotMeasured instead. Got {:?}",
            c.key,
            b.ascii_two_point
        );
        assert!(
            !b.unmodelled.is_empty(),
            "{}: a byte-budget cell must say what the budget covers beyond the body, because \
             that is why even a saturating arm is an upper bound rather than the boundary",
            c.key
        );
        // The saturating arm is the one that CAN decide it. Undriven is fine
        // and is the truth today; silently absent is not.
        if let Saturating::NotDriven { waiting_on } = b.saturating {
            assert!(
                waiting_on.contains("astral"),
                "{}: the blocker must name the arm that would settle this shape — astral-plane \
                 scalars at the cap — not just the credential a two-point probe would have \
                 wanted. Got: {waiting_on}",
                c.key
            );
        }
    }
    assert_eq!(
        seen, 2,
        "expected matrix and msteams to be byte-budget cells; a shrinking count means one \
         regressed to a shape its probe cannot decide"
    );
}

/// **wayland#934 c8.** A settled unit verdict must be enforced against the
/// shipped cap, not merely recorded beside it.
///
/// Telegram was the sharp case and it is SETTLED as of 2026-08-29: the astral
/// arm was driven at the real bot and group, 4,096 U+1F600 scalars — 8,192
/// UTF-16 code units — were accepted AT the shipped cap and 4,097 refused one
/// scalar later, so a 4,096 code-unit limit is refuted and Telegram counts
/// SCALARS. No cell records a UTF-16 verdict, so the halving rule still has
/// nothing live to enforce — which is exactly the state that makes a rule rot.
/// The enforcement is therefore exercised in the same run by
/// [`the_unit_rule_refuses_a_cap_a_utf16_verdict_makes_unsafe`] over the same
/// function, and the census below keeps the owed runs visible.
#[test]
fn a_settled_unit_verdict_is_enforced_against_the_shipped_cap() {
    let caps = shipped_caps();
    let mut unsettled: Vec<&str> = Vec::new();
    for (key, cap) in &caps {
        let Boundary::Measured { unit, .. } = cell(key).boundary else {
            continue;
        };
        let faults = unit_safety_faults(*cap, &unit);
        assert!(
            faults.is_empty(),
            "{key}: the shipped cap is unsafe under its own measured unit:\n  {}",
            faults.join("\n  ")
        );
        if unit.is_unsettled() {
            unsettled.push(key);
        }
    }
    println!(
        "UNSETTLED_CAP_UNITS count={} — an ASCII probe cannot tell characters from UTF-16 code \
         units, because in ASCII they are the same number",
        unsettled.len()
    );
    for key in &unsettled {
        let Boundary::Measured {
            unit: CapUnit::UnsettledAsciiOnly { needs },
            ..
        } = cell(key).boundary
        else {
            continue;
        };
        println!("  UNSETTLED {key} needs: {needs}");
    }

    // wayland#934 c8, settled by the run recorded on the cell. Assert the
    // verdict is RECORDED, because losing it is otherwise invisible here:
    // `unit_safety_faults` is silent on an unsettled cell by design, so
    // reverting telegram to `UnsettledAsciiOnly` would keep every assertion
    // above green while the product went back to shipping an unverified unit.
    let Boundary::Measured { unit, .. } = cell("telegram").boundary else {
        panic!("telegram must have a measured boundary");
    };
    assert!(
        matches!(unit, CapUnit::MeasuredScalars { .. }),
        "telegram's unit question was settled in SCALARS by a live astral run on 2026-08-29 \
         (4,096 scalars = 8,192 UTF-16 code units accepted at the cap; 4,097 refused). Got: \
         {unit:?}"
    );

    // And a scalar verdict only means something if the run FOUND the boundary.
    // An astral arm accepted on both sides settles nothing, so a
    // `MeasuredScalars` cell whose over-arm is not a refusal is a verdict with
    // no boundary behind it.
    for c in CELLS {
        let Boundary::Measured { unit, above, .. } = c.boundary else {
            continue;
        };
        if matches!(unit, CapUnit::MeasuredScalars { .. }) {
            assert!(
                matches!(above, Above::Refused(_)),
                "{}: the unit is recorded as MeasuredScalars, but the over-arm is {above:?} — a \
                 run that never found the boundary cannot settle the unit",
                c.key
            );
        }
    }
}

/// **Can it fail** — the unit rule, driven over a settled verdict.
///
/// Constructs the exact verdict a Telegram astral run would produce if the
/// platform turns out to count UTF-16 code units, and requires the checker to
/// refuse the cap the product ships today. Without this arm the rule would be
/// an unexercised branch, and an unexercised rule is indistinguishable from an
/// absent one.
#[test]
fn the_unit_rule_refuses_a_cap_a_utf16_verdict_makes_unsafe() {
    let utf16 = CapUnit::MeasuredUtf16CodeUnits {
        limit_code_units: 4_096,
        on: "hypothetical",
        evidence: "the shape a Telegram astral run would produce if the limit is code units",
    };

    // Known-positive: at half the limit the same checker is silent, so a red
    // below is the rule firing rather than the checker reporting everything.
    assert!(unit_safety_faults(2_048, &utf16).is_empty());

    let faults = unit_safety_faults(4_096, &utf16);
    assert_eq!(faults.len(), 1, "got: {faults:?}");
    assert!(
        faults[0].contains("8192") && faults[0].contains("2048"),
        "the fault must name the code units the body costs and the largest safe scalar count: \
         {faults:?}"
    );

    // The other settled verdict must NOT produce a fault: a scalar limit means
    // the cap is the cap whatever the encoding.
    let scalars = CapUnit::MeasuredScalars {
        on: "hypothetical",
        evidence: "the shape the same run would produce if the limit is characters",
    };
    assert!(
        unit_safety_faults(4_096, &scalars).is_empty(),
        "a scalar verdict must not be read as a code-unit one"
    );

    // And an ASCII-only run must invent neither a pass nor a fault.
    let ascii = CapUnit::UnsettledAsciiOnly { needs: "the run" };
    assert!(unit_safety_faults(999_999, &ascii).is_empty());
}

/// The word `live` in `docs/delivery-semantics.md` must point at a committed
/// probe, in both directions.
///
/// Before this file existed, `slack.cap_measured = live` and
/// `discord.cap_measured = live` rested on a date in a prose table and a
/// hardcoded `matches!(*p, "slack" | "discord")` exemption inside
/// `delivery_semantics_declaration.rs`. That is a claim of measurement with no
/// artifact: nothing in the tree could be re-run, and nothing would notice if
/// the exemption outlived the run it was written for.
#[test]
fn every_live_verdict_in_the_declaration_has_a_probe_cell_here() {
    let live = keys_claiming_live();
    assert!(
        !live.is_empty(),
        "parsed zero `cap_measured = live` rows. If every verdict really did go back to `no`, \
         the Measured cells in this file are now the unsupported claim and must go too."
    );
    for key in &live {
        assert!(
            matches!(cell(key).boundary, Boundary::Measured { .. }),
            "{key}: docs/delivery-semantics.md says cap_measured = live, but this file has no \
             measured cell for it. `live` may only be written for a boundary a committed probe \
             records and can re-derive."
        );
    }
    // And the reverse: a measured cell here that the document still calls
    // unmeasured means one of the two was updated and the other forgotten.
    for c in CELLS {
        if matches!(c.boundary, Boundary::Measured { .. }) {
            assert!(
                live.iter().any(|p| p == c.key),
                "{}: this file records a measured boundary but the declaration does not say \
                 cap_measured = live. Record the run in §4.2 in the same commit.",
                c.key
            );
        }
    }
}

/// **wayland-core#360 c2.** A capped implementation with no cell here would be
/// a cap nothing in this file knows is unmeasured.
///
/// The loop walks `constructible_selectors()`, so it reaches the WhatsApp
/// bridge — `whatsapp` plus `backend = "baileys"` — which the platform-keyed
/// version could not. That is the whole point: the guard that exists to make an
/// unprobed cap impossible had a blind spot shaped exactly like the eighth
/// `max_message_len` in the product, and measuring one number without widening
/// the guard would only have moved the blind spot.
#[test]
fn every_capped_adapter_has_a_probe_cell() {
    let caps = shipped_caps();
    assert!(
        caps.iter().any(|(k, _)| k.contains('+')),
        "no config-key-selected implementation reached this guard, so the widening is not doing \
         anything. `whatsapp+baileys` must be in {:?}",
        caps.iter().map(|(k, _)| k).collect::<Vec<_>>()
    );
    for (key, _) in caps {
        assert!(
            CELLS.iter().any(|c| c.key == key),
            "{key} declares a finite max_message_len() but has no cell in this file. Add one — \
             as NotMeasured naming the blocker if nobody can drive it — so the gap is stated \
             rather than absent."
        );
    }
}

/// Every cell must name something the registry can actually build.
///
/// The converse of the guard above, and it is not decoration: a cell whose key
/// no selector answers to would satisfy every completeness check while probing
/// nothing, and `cell()`'s panic would only fire on the live path nobody runs.
#[test]
fn every_cell_names_a_selector_the_registry_can_build() {
    let keys: Vec<String> = constructible_selectors()
        .into_iter()
        .map(|s| s.key)
        .collect();
    for c in CELLS {
        assert!(
            keys.contains(&c.key.to_string()),
            "{} has a cell here but no selector: the registry cannot build it, so nothing this \
             cell says can be checked. Known selectors: {keys:?}",
            c.key
        );
        assert!(
            channel_factory_for(c.platform()).is_some(),
            "{}: platform {:?} has no factory",
            c.key,
            c.platform()
        );
    }
}

/// **Not `#[ignore]`d, on purpose.** The visible remainder.
///
/// `cargo test --test live_message_cap_boundary` exits 0 having driven zero
/// live cells and printing `test result: ok`, which is indistinguishable from
/// having measured all nine. This prints the census instead, so an unmeasured
/// cap is loud rather than absent.
#[test]
fn unprobed_caps_are_visibly_unrun() {
    let unrun: Vec<&Cell> = CELLS
        .iter()
        .filter(|c| {
            matches!(
                c.boundary,
                Boundary::NotMeasured { .. }
                    | Boundary::Derived(ByteBudget {
                        saturating: Saturating::NotDriven { .. },
                        ..
                    })
            )
        })
        .collect();
    assert!(
        !unrun.is_empty(),
        "the unrun census is empty. Either every boundary was driven and each cell should say \
         so with its numbers, or the cells were deleted — and deleting an unrun measurement is \
         how it stops being visibly unrun."
    );
    println!(
        "UNRUN_CAP_BOUNDARIES count={} of {} — a skip is NOT a pass",
        unrun.len(),
        CELLS.len()
    );
    for c in unrun {
        let (waiting_on, shape) = match c.boundary {
            Boundary::NotMeasured { waiting_on } => (waiting_on, "two-point character boundary"),
            Boundary::Derived(ByteBudget {
                saturating: Saturating::NotDriven { waiting_on },
                ..
            }) => (waiting_on, "SATURATING arm at the worst-case encoding"),
            _ => continue,
        };
        println!("  UNRUN selector={} gated_on={}", c.key, c.env.join(" + "));
        println!("        shape: {shape}");
        println!("        needs: {waiting_on}");
    }
    println!(
        "Until each prints a LIVE_CAP_VERDICT line, docs/delivery-semantics.md must keep saying \
         cap_measured = no for these rows."
    );
}

macro_rules! live_cell {
    ($name:ident, $key:literal, $to_var:literal, $why:literal) => {
        #[tokio::test]
        #[ignore = $why]
        async fn $name() {
            let to = required($to_var);
            drive_cell($key, &to).await;
        }
    };
}

live_cell!(
    live_boundary_at_real_slack,
    "slack",
    "WL_LIVE_CAP_SLACK_TO",
    "posts a 4,040-character message to a real Slack workspace"
);
live_cell!(
    live_boundary_at_real_discord,
    "discord",
    "WL_LIVE_CAP_DISCORD_TO",
    "posts a 2,000-character message to a real Discord channel"
);
live_cell!(
    live_boundary_at_real_matrix,
    "matrix",
    "WL_LIVE_CAP_MATRIX_TO",
    "needs a live homeserver token; drives the SATURATING arm, not the two-point probe"
);
live_cell!(
    live_boundary_at_real_telegram,
    "telegram",
    "WL_LIVE_CAP_TELEGRAM_TO",
    "posts a 4,096-character message to a real Telegram chat; set \
     WL_LIVE_CAP_TELEGRAM_ASTRAL=1 to settle the unit question"
);
live_cell!(
    live_boundary_at_real_twilio_sms,
    "sms",
    "WL_LIVE_CAP_SMS_TO",
    "needs a Twilio credential, and every send is billable"
);
live_cell!(
    live_boundary_at_real_whatsapp,
    "whatsapp",
    "WL_LIVE_CAP_WHATSAPP_TO",
    "needs a Meta business credential nobody on the programme holds"
);
live_cell!(
    live_boundary_at_real_msteams,
    "msteams",
    "WL_LIVE_CAP_MSTEAMS_TO",
    "needs a Bot Framework app registration; drives the SATURATING arm, not the two-point probe"
);
live_cell!(
    live_boundary_at_real_whatsapp_baileys_bridge,
    "whatsapp+baileys",
    "WL_LIVE_CAP_WHATSAPP_BAILEYS_TO",
    "needs a running bridge.js with @whiskeysockets/baileys and a QR-paired number"
);
live_cell!(
    live_boundary_at_real_whatsapp_web_bridge,
    "whatsapp+whatsapp-web",
    "WL_LIVE_CAP_WHATSAPP_WEB_TO",
    "needs a running bridge.js with whatsapp-web.js, a Chromium, and a QR-paired number"
);
