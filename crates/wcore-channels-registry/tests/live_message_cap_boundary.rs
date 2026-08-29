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
//! # What a boundary is here, and why it is not always `cap + 1`
//!
//! The probe drives two sends at a real destination: one of exactly
//! [`Cell::accepts_up_to`] characters, which must arrive as ONE message, and
//! one of `accepts_up_to + 1`, which must do whatever [`Above`] records for
//! that platform.
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
//! # A probe that cannot run must not read as a pass
//!
//! Two of the seven cells have ever been driven. The other five need a
//! credential nobody on the programme holds, and each says which one.
//!
//! * The live cells are `#[ignore]`d and every one of them **panics naming the
//!   missing variable** when invoked with `--ignored` and an incomplete
//!   environment. An env-gated `return` is the shape that printed `5 passed`
//!   for zero work in `live_integrity.rs`.
//! * [`unprobed_caps_are_visibly_unrun`] is **not** `#[ignore]`d. It prints the
//!   census of never-driven cells on every ordinary `cargo test`, so "nobody
//!   has ever measured this" stays visible in CI output instead of living in a
//!   doc comment somebody has to remember.
//! * [`a_shipped_cap_never_exceeds_its_measured_boundary`] and
//!   [`every_live_verdict_in_the_declaration_has_a_probe_cell_here`] are not
//!   `#[ignore]`d either. They are the part of the measurement that survives
//!   the run: the recorded boundary is checked against the shipped constant,
//!   and the word `live` in `docs/delivery-semantics.md` is bound to a cell in
//!   this file rather than to a sentence.
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

use std::sync::Arc;

use wcore_channels::outgoing::OutgoingMessage;
use wcore_channels::{Channel, ChannelConfig};
use wcore_channels_registry::channel_factory_for;
use wcore_config::credentials::{CredentialsStore, PlaintextCredentialsStore};

/// The declaration, read from source at test time. `include_str!` rather than a
/// runtime path read, for the same reason
/// `delivery_semantics_declaration.rs` does it: a wrong path is then a compile
/// error rather than a silent zero-row parse that makes every assertion vacuous.
const DECLARATION: &str = include_str!("../../../docs/delivery-semantics.md");

/// What the platform did to a body one character above [`Cell::accepts_up_to`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Above {
    /// The platform returned an error the adapter surfaces as a
    /// `ChannelError`. The string is the platform's own diagnostic, recorded so
    /// a future run can tell "the boundary moved" apart from "the credential
    /// broke".
    Refused(&'static str),
    /// The platform accepted the request and did NOT deliver one message —
    /// it reshaped the body. There is no error to catch, which is precisely
    /// why the shipped cap sits below this number.
    SilentlyReshaped(&'static str),
}

/// One platform's boundary, as measured at the platform.
#[derive(Debug, Clone, Copy)]
enum Boundary {
    /// Driven at the real destination on `on` (ISO date).
    ///
    /// `accepts_up_to` is NOT a constant this repository returns from anywhere.
    /// It is an observation of a third party, recorded here so the shipped
    /// constant has something outside itself to be checked against, and
    /// re-derived by the live cell every time one runs.
    Measured {
        accepts_up_to: usize,
        above: Above,
        on: &'static str,
    },
    /// Never driven. `waiting_on` names the credential, not an excuse.
    NotMeasured { waiting_on: &'static str },
}

/// One capped adapter's probe cell.
struct Cell {
    /// Platform tag as `channel_factory_for` spells it.
    platform: &'static str,
    boundary: Boundary,
    /// The three variables the live cell requires, in the order the panic names
    /// them.
    env: [&'static str; 3],
}

impl Cell {
    /// The length the live probe sends as its "accepted" arm.
    ///
    /// For a measured platform that is the recorded boundary, so a re-run
    /// re-derives the same fact. For an unmeasured one there is no boundary
    /// yet, so the probe drives the DECLARED cap and reports what happened —
    /// which is the discovery run that turns `NotMeasured` into `Measured`.
    fn probe_at(&self, declared_cap: usize) -> usize {
        match self.boundary {
            Boundary::Measured { accepts_up_to, .. } => accepts_up_to,
            Boundary::NotMeasured { .. } => declared_cap,
        }
    }
}

/// Every adapter that declares a finite `max_message_len()`.
///
/// The set is checked against the registry by
/// [`every_capped_adapter_has_a_probe_cell`], so a new capped adapter cannot be
/// added without either probing it or saying out loud that nobody has.
const CELLS: &[Cell] = &[
    Cell {
        platform: "slack",
        boundary: Boundary::Measured {
            // 4,040 is the largest body that stayed one message. The shipped
            // cap is 4,000 — Slack's own documented "for best results" figure
            // and the point its splitter uses — which is BELOW the boundary and
            // therefore safe.
            accepts_up_to: 4_040,
            above: Above::SilentlyReshaped(
                "4,041 is accepted and split by the API into 4,000-char messages; no error",
            ),
            on: "2026-08-27",
        },
        env: [
            "WL_LIVE_CAP_SLACK_HOME",
            "WL_LIVE_CAP_SLACK_CHANNEL",
            "WL_LIVE_CAP_SLACK_TO",
        ],
    },
    Cell {
        platform: "discord",
        boundary: Boundary::Measured {
            accepts_up_to: 2_000,
            above: Above::Refused("HTTP 400, code 50035 Invalid Form Body"),
            on: "2026-08-27",
        },
        env: [
            "WL_LIVE_CAP_DISCORD_HOME",
            "WL_LIVE_CAP_DISCORD_CHANNEL",
            "WL_LIVE_CAP_DISCORD_TO",
        ],
    },
    Cell {
        platform: "matrix",
        boundary: Boundary::NotMeasured {
            waiting_on: "a live homeserver access token. The one this programme held was found \
                         dead on 2026-07-31 (M_UNKNOWN_TOKEN) and has not been reissued. The \
                         16,384 figure is DERIVED from a 65,536-BYTE PDU limit at four bytes \
                         per scalar, so the real boundary depends on the body's encoding and \
                         cannot be computed by the client at all",
        },
        env: [
            "WL_LIVE_CAP_MATRIX_HOME",
            "WL_LIVE_CAP_MATRIX_CHANNEL",
            "WL_LIVE_CAP_MATRIX_TO",
        ],
    },
    Cell {
        platform: "telegram",
        boundary: Boundary::NotMeasured {
            waiting_on: "a Telegram bot token and a chat the bot is a member of. Telegram \
                         documents 4,096 characters but indexes entities in UTF-16 code units \
                         on the same page, so which unit the 4,096 counts is unmeasured",
        },
        env: [
            "WL_LIVE_CAP_TELEGRAM_HOME",
            "WL_LIVE_CAP_TELEGRAM_CHANNEL",
            "WL_LIVE_CAP_TELEGRAM_TO",
        ],
    },
    Cell {
        platform: "sms",
        boundary: Boundary::NotMeasured {
            waiting_on: "a Twilio account SID, auth token and provisioned From number. No \
                         Twilio credential exists on this programme (measured, see the \
                         2026-07-30 correction in docs/delivery-semantics.md), and every probe \
                         send is billable",
        },
        env: [
            "WL_LIVE_CAP_SMS_HOME",
            "WL_LIVE_CAP_SMS_CHANNEL",
            "WL_LIVE_CAP_SMS_TO",
        ],
    },
    Cell {
        platform: "whatsapp",
        boundary: Boundary::NotMeasured {
            waiting_on: "a Meta Business app with the WhatsApp product, a phone-number id, a \
                         system-user access token, and a recipient inside the 24-hour \
                         customer-service window. No Meta credential exists on this programme",
        },
        env: [
            "WL_LIVE_CAP_WHATSAPP_HOME",
            "WL_LIVE_CAP_WHATSAPP_CHANNEL",
            "WL_LIVE_CAP_WHATSAPP_TO",
        ],
    },
    Cell {
        platform: "msteams",
        boundary: Boundary::NotMeasured {
            waiting_on: "a registered Bot Framework app id + password and a Teams tenant that \
                         will accept it. Microsoft documents NO character limit for a bot \
                         message — only an 80-100 KB UTF-16 payload budget covering the whole \
                         Activity — so 20,480 is derived and the real boundary depends on \
                         @-mentions and attachment JSON the client does not control",
        },
        env: [
            "WL_LIVE_CAP_MSTEAMS_HOME",
            "WL_LIVE_CAP_MSTEAMS_CHANNEL",
            "WL_LIVE_CAP_MSTEAMS_TO",
        ],
    },
];

fn cell(platform: &str) -> &'static Cell {
    CELLS
        .iter()
        .find(|c| c.platform == platform)
        .unwrap_or_else(|| panic!("no probe cell for platform {platform:?}"))
}

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

/// Hermetic per-platform config, sufficient to construct the adapter. Mirrors
/// `delivery_semantics_declaration.rs`'s fixtures; no adapter contacts a network
/// during construction.
fn fixture_options(platform: &str) -> toml::Table {
    let body: &str = match platform {
        "slack" => {
            "workspace_name = \"fixture\"\n\
             credential_handle_bot_token = \"fixture.slack.bot_token\"\n\
             credential_handle_signing_secret = \"fixture.slack.signing_secret\"\n"
        }
        "telegram" => "credential_handle = \"fixture.telegram.bot_token\"\n",
        "discord" => "credential_handle = \"fixture.discord.bot_token\"\n",
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
            "no fixture config for platform {other:?}; a new capped adapter needs one here as \
             well as a probe cell"
        ),
    };
    toml::from_str(body).expect("fixture config must parse")
}

/// Build every adapter the registry can construct and return the ones that
/// declare a finite cap, as the SHIPPED BINARY builds them (through
/// `channel_factory_for`, the factory `auto_register_from_dir` uses).
fn shipped_caps() -> Vec<(String, usize)> {
    // The platform set is taken from the declaration rather than hand-listed,
    // so a platform added to the product and to the document but not to this
    // file is caught by `every_capped_adapter_has_a_probe_cell` below.
    let mut out = Vec::new();
    for platform in declared_platforms() {
        let Some(factory) = channel_factory_for(&platform) else {
            continue;
        };
        let ch = factory(
            format!("fixture-{platform}"),
            &fixture_options(&platform),
            Arc::new(MemStore),
        )
        .unwrap_or_else(|e| panic!("could not construct {platform:?} from its fixture: {e}"));
        if let Some(cap) = ch.max_message_len() {
            out.push((platform, cap));
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

/// Platforms carrying a `<platform>.cap` row.
fn declared_platforms() -> Vec<String> {
    let mut out: Vec<String> = machine_block()
        .lines()
        .filter_map(|l| l.split_once('='))
        .filter_map(|(k, _)| k.trim().strip_suffix(".cap").map(str::to_string))
        .collect();
    out.sort();
    out.dedup();
    assert!(
        !out.is_empty(),
        "parsed zero cap rows out of the declaration, which would make every assertion below \
         vacuous"
    );
    out
}

/// Platforms whose `<platform>.cap_measured` verdict is `live`.
fn platforms_claiming_live() -> Vec<String> {
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
/// For every platform whose boundary has actually been driven, the cap the
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
    for (platform, cap) in &caps {
        let Boundary::Measured {
            accepts_up_to,
            above,
            on,
        } = cell(platform).boundary
        else {
            continue;
        };
        checked += 1;
        assert!(
            *cap <= accepts_up_to,
            "{platform}: the shipped cap is {cap} but the platform accepted at most \
             {accepts_up_to} chars as a single message (measured {on}; above it: {above:?}). A \
             cap over the boundary is HIGH-6 — send_to_keyed hands the platform a body it will \
             not deliver whole, and nothing re-sends it."
        );
    }
    assert!(
        checked > 0,
        "no platform in CELLS is Measured, so this test asserted nothing. Either a boundary run \
         was recorded and this is a parsing bug, or the measured cells were deleted — and \
         deleting a measurement is how it stops being visibly present."
    );
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
    let live = platforms_claiming_live();
    assert!(
        !live.is_empty(),
        "parsed zero `cap_measured = live` rows. If every verdict really did go back to `no`, \
         the Measured cells in this file are now the unsupported claim and must go too."
    );
    for platform in &live {
        assert!(
            matches!(cell(platform).boundary, Boundary::Measured { .. }),
            "{platform}: docs/delivery-semantics.md says cap_measured = live, but this file has \
             no measured cell for it. `live` may only be written for a boundary a committed \
             probe records and can re-derive."
        );
    }
    // And the reverse: a measured cell here that the document still calls
    // unmeasured means one of the two was updated and the other forgotten.
    for c in CELLS {
        if matches!(c.boundary, Boundary::Measured { .. }) {
            assert!(
                live.iter().any(|p| p == c.platform),
                "{}: this file records a measured boundary but the declaration does not say \
                 cap_measured = live. Record the run in §4.2 in the same commit.",
                c.platform
            );
        }
    }
}

/// A capped adapter with no cell here would be a cap nothing in this file knows
/// is unmeasured — which is how the seven silently became six.
#[test]
fn every_capped_adapter_has_a_probe_cell() {
    for (platform, _) in shipped_caps() {
        assert!(
            CELLS.iter().any(|c| c.platform == platform),
            "{platform} declares a finite max_message_len() but has no cell in this file. Add \
             one — as NotMeasured naming the credential if nobody can drive it — so the gap is \
             stated rather than absent."
        );
    }
}

/// **Not `#[ignore]`d, on purpose.** The visible remainder.
///
/// `cargo test --test live_message_cap_boundary` exits 0 having driven zero
/// live cells and printing `test result: ok`, which is indistinguishable from
/// having measured all seven. This prints the census instead, so an unmeasured
/// cap is loud rather than absent.
#[test]
fn unprobed_caps_are_visibly_unrun() {
    let unrun: Vec<&Cell> = CELLS
        .iter()
        .filter(|c| matches!(c.boundary, Boundary::NotMeasured { .. }))
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
        let Boundary::NotMeasured { waiting_on } = c.boundary else {
            continue;
        };
        println!(
            "  UNRUN platform={} gated_on={}",
            c.platform,
            c.env.join(" + ")
        );
        println!("        needs: {waiting_on}");
    }
    println!(
        "Until each prints a LIVE_CAP_VERDICT line, docs/delivery-semantics.md must keep saying \
         cap_measured = no for these rows."
    );
}

// ---------------------------------------------------------------------------
// The live cells
// ---------------------------------------------------------------------------

/// Required env var, or a loud failure. Never a silent skip.
fn required(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "{var} is not set. This test was invoked with --ignored, which means a live run was \
             explicitly requested; returning quietly would print a pass for zero work."
        ),
    }
}

/// Build and start the adapter through the production construction chain.
///
/// This is `auto_register_from_dir`'s body with the manager left out: the same
/// `parse_channel_config`, the same [`channel_factory_for`], the same
/// `factory(..)`, the same on-disk `channels/<name>.toml` and
/// `credentials.toml` that `gateway run` reads. The manager is omitted for one
/// reason, stated at the top of this file: its send chunks, and a chunked send
/// measures our chunker rather than the platform.
async fn production_adapter(home: &str, name: &str, platform: &str) -> Box<dyn Channel> {
    let path = std::path::Path::new(home)
        .join("channels")
        .join(format!("{name}.toml"));
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    let cfg: ChannelConfig = wcore_channels::parse_channel_config(name, &body)
        .unwrap_or_else(|e| panic!("{} did not parse: {e}", path.display()));
    assert_eq!(
        cfg.platform,
        platform,
        "{} declares platform {:?} but this cell probes {platform:?}; probing the wrong adapter \
         would record one platform's boundary against another's cap",
        path.display(),
        cfg.platform
    );
    let creds: Arc<dyn CredentialsStore> = Arc::new(PlaintextCredentialsStore::new(
        std::path::Path::new(home).join("credentials.toml"),
    ));
    let factory = channel_factory_for(&cfg.platform)
        .unwrap_or_else(|| panic!("registry has no factory for {:?}", cfg.platform));
    let mut ch = factory(cfg.name.clone(), &cfg.options, creds)
        .unwrap_or_else(|e| panic!("could not construct {:?}: {e}", cfg.platform));
    ch.start()
        .await
        .expect("start() must succeed — constructing does NOT authenticate");
    ch
}

/// The shared body of every live cell.
///
/// Sends a body of exactly `at` characters (must arrive as ONE message) and one
/// of `at + 1` (must do what the cell records, or — for an unmeasured platform
/// — is reported so the run can record it). Deletes what it posted where the
/// adapter supports deletion.
async fn drive_boundary(platform: &str, to: &str) {
    let c = cell(platform);
    let [home_var, chan_var, to_var] = c.env;
    let _ = (home_var, chan_var, to_var);

    let home = required(home_var);
    let name = required(chan_var);
    let mut ch = production_adapter(&home, &name, platform).await;

    let declared = ch.max_message_len().unwrap_or_else(|| {
        panic!("{platform} declares no cap; there is no boundary to probe and chunking is off")
    });
    let at = c.probe_at(declared);
    println!("LIVE_CAP_PROBE platform={platform} shipped_cap={declared} probing_at={at}");

    // A single unbroken ASCII run: one char is one scalar is one byte, so the
    // length the platform sees is unambiguous. A multi-byte body would make a
    // refusal readable as either a character limit or a byte limit, and this
    // probe would not be able to say which.
    let at_boundary = "x".repeat(at);
    let first = ch
        .send_message(OutgoingMessage::text(to.to_string(), at_boundary.clone()))
        .await;
    match &first {
        Ok(r) => println!("LIVE_CAP_AT     len={at} accepted platform_id={}", r.id),
        Err(e) => println!("LIVE_CAP_AT     len={at} REFUSED {e}"),
    }

    let over = format!("{at_boundary}y");
    let second = ch
        .send_message(OutgoingMessage::text(to.to_string(), over))
        .await;
    match &second {
        Ok(r) => println!(
            "LIVE_CAP_OVER   len={} accepted platform_id={}",
            at + 1,
            r.id
        ),
        Err(e) => println!("LIVE_CAP_OVER   len={} REFUSED {e}", at + 1),
    }

    // Clean up before asserting, so a failing assertion does not strand
    // multi-thousand-character messages in a real destination.
    for receipt in [first.as_ref().ok(), second.as_ref().ok()]
        .into_iter()
        .flatten()
    {
        match ch.delete_message(to, &receipt.id).await {
            Ok(()) => println!("LIVE_CAP_CLEAN  deleted {}", receipt.id),
            Err(e) => println!("LIVE_CAP_CLEAN  could NOT delete {} — {e}", receipt.id),
        }
    }

    // The at-boundary send is the instrument. A measurement taken after it
    // failed is not a boundary result, it is no result.
    let first = first.unwrap_or_else(|e| {
        panic!(
            "INSTRUMENT_FAULT: {platform} refused a body of {at} chars — {e}. Nothing below this \
             is a boundary measurement: grade the run INCOMPLETE, not a narrower boundary. A \
             dead credential and a moved boundary produce the same red here, and only the \
             platform's own diagnostic tells them apart."
        )
    });
    assert!(
        !first.id.is_empty(),
        "INSTRUMENT_FAULT: the platform returned an empty message id, so nothing can be \
         corroborated at its console"
    );

    match c.boundary {
        Boundary::Measured {
            above: Above::Refused(evidence),
            ..
        } => {
            let err = second.err().unwrap_or_else(|| {
                panic!(
                    "{platform} ACCEPTED {} chars. It refused that on the recorded run \
                     ({evidence}), so the boundary has MOVED — re-measure it, update the cell, \
                     and only then consider raising the shipped cap.",
                    at + 1
                )
            });
            println!("LIVE_CAP_VERDICT platform={platform} boundary={at} above=refused ({err})");
        }
        Boundary::Measured {
            above: Above::SilentlyReshaped(evidence),
            ..
        } => {
            // There is no error to assert on: the platform takes the body and
            // reshapes it. Reported, not asserted, and the operator corroborates
            // at the platform's own console — an id is our read of their
            // response, whereas the claim is about what a human sees in the
            // channel, and those are the two claims this repository has already
            // conflated once.
            println!(
                "LIVE_CAP_VERDICT platform={platform} boundary={at} above=silently-reshaped \
                 ({evidence}) — CORROBORATE AT THE PLATFORM: count the messages that arrived \
                 for the over-boundary send before believing this row."
            );
        }
        Boundary::NotMeasured { .. } => {
            println!(
                "LIVE_CAP_VERDICT platform={platform} DISCOVERY at shipped cap {at}: \
                 over_boundary_accepted={}. This cell is still NotMeasured — record the numbers \
                 above in docs/delivery-semantics.md §4.2 and convert the cell in the same \
                 commit.",
                second.is_ok()
            );
        }
    }
}

macro_rules! live_cell {
    ($name:ident, $platform:literal, $to_var:literal, $why:literal) => {
        #[tokio::test]
        #[ignore = $why]
        async fn $name() {
            let to = required($to_var);
            drive_boundary($platform, &to).await;
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
    "needs a live homeserver token; the programme's was dead on 2026-07-31"
);
live_cell!(
    live_boundary_at_real_telegram,
    "telegram",
    "WL_LIVE_CAP_TELEGRAM_TO",
    "needs a Telegram bot token nobody on the programme holds"
);
live_cell!(
    live_boundary_at_real_twilio_sms,
    "sms",
    "WL_LIVE_CAP_SMS_TO",
    "needs a Twilio credential nobody holds, and every send is billable"
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
    "needs a Bot Framework app registration and a Teams tenant nobody holds"
);
