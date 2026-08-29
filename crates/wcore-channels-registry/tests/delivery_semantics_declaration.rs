//! `docs/delivery-semantics.md` must not drift from the adapters it describes.
//!
//! # Why this test exists
//!
//! Phase 24, criterion `24-C1`. The per-adapter delivery guarantee is a
//! customer-facing promise, and a promise that lives only in a markdown file is
//! the same defect class as a security flag whose code does not enforce it: a
//! reassuring sentence over behaviour that may already have changed underneath
//! it. Three adapters declare `supports_outbound_idempotency() == true` and
//! seven inherit `false`; nothing previously stopped that ratio changing — in
//! either direction — while the document kept saying what it said.
//!
//! So the document is parsed here and compared against the adapters as the
//! SHIPPED BINARY builds them: through `channel_factory_for`, the same
//! production factory `auto_register_from_dir` uses. An adapter constructed any
//! other way would not be evidence about the product.
//!
//! # Both directions, per LANE-BRIEF §3.2 and §3b-iii
//!
//! A gate is worth nothing unless it can fail AND can pass.
//!
//! * **Can pass** — [`declaration_matches_every_adapter`] is the known-positive:
//!   the real document against the real adapters, green today.
//! * **Can fail** — [`comparator_rejects_a_flipped_row`] and
//!   [`comparator_rejects_a_missing_row`] run the SAME comparator over mutated
//!   inputs and assert it reports the mismatch. They are not hypothetical
//!   reasoning about what would happen; they execute it in this run.
//!
//! The completeness check matters as much as the per-row one: a new adapter
//! added to the registry with no row in the document is precisely the drift a
//! row-by-row comparison alone would miss, because there is no row to disagree
//! with.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use wcore_channels_registry::channel_factory_for;
use wcore_config::credentials::{CredentialsError, CredentialsStore};

/// The declaration, read from source at test time.
///
/// `include_str!` rather than a runtime path read: the test binary then carries
/// the document, so it cannot pass by failing to find the file. A wrong path is
/// a compile error, not a silent zero-row parse that would make every
/// assertion below vacuous.
const DECLARATION: &str = include_str!("../../../docs/delivery-semantics.md");

const BEGIN: &str = "<!-- DELIVERY-SEMANTICS-MACHINE-READABLE";
const END: &str = "-->";

/// In-memory credentials store. No adapter reads a credential during
/// construction, but the factory signature requires one, and reaching for the
/// real keyring in a test would touch the developer's own secrets.
struct MemStore(Mutex<std::collections::HashMap<String, String>>);

impl CredentialsStore for MemStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

fn creds() -> Arc<dyn CredentialsStore> {
    Arc::new(MemStore(Mutex::new(Default::default())))
}

/// Minimal valid options per platform — every required field, nothing more.
///
/// No real credential, no reachable host: each `credential_handle_*` names a
/// key that does not exist in the `MemStore`, and construction does not resolve
/// it. Endpoint fields are left at their defaults because nothing here connects.
fn fixture_options(platform: &str) -> toml::Table {
    let body: &str = match platform {
        "slack" => {
            "workspace_name = \"fixture\"\n\
             credential_handle_bot_token = \"fixture.slack.bot_token\"\n\
             credential_handle_signing_secret = \"fixture.slack.signing_secret\"\n"
        }
        "telegram" => "credential_handle = \"fixture.telegram.bot_token\"\n",
        "email" => {
            "from_address = \"bot@fixture.invalid\"\n\
             [smtp]\n\
             host = \"smtp.fixture.invalid\"\n\
             user_credential_handle = \"fixture.email.smtp_user\"\n\
             password_credential_handle = \"fixture.email.smtp_pass\"\n"
        }
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
        "signal" => "account = \"+15550000000\"\n",
        "matrix" => {
            "homeserver_url = \"https://matrix.fixture.invalid\"\n\
             credential_handle_access_token = \"fixture.matrix.access_token\"\n\
             user_id = \"@fixture-bot:fixture.invalid\"\n"
        }
        "msteams" => {
            "credential_handle_app_id = \"fixture.msteams.app_id\"\n\
             credential_handle_app_password = \"fixture.msteams.app_password\"\n"
        }
        // Every field is `#[serde(default)]`.
        "imessage" => "",
        other => panic!(
            "no fixture config for platform {other:?}. A new adapter needs one here AND a row \
             in docs/delivery-semantics.md — that is what this test is for."
        ),
    };
    toml::from_str(body).expect("fixture config must parse")
}

/// The platforms this build can construct.
///
/// iMessage is `#[cfg(target_os = "macos")]` in the registry, so the expected
/// set is genuinely platform-dependent and the document says so. Deriving the
/// list here with the same `cfg` keeps the two in step.
fn constructible_platforms() -> Vec<&'static str> {
    let mut v = vec![
        "slack", "telegram", "email", "discord", "sms", "whatsapp", "signal", "matrix", "msteams",
    ];
    if cfg!(target_os = "macos") {
        v.push("imessage");
    }
    v.sort_unstable();
    v
}

/// Parse the machine-readable block out of the declaration.
///
/// Returns `platform -> guarantee`. Deliberately strict: an unparseable or
/// absent block panics rather than yielding an empty map, because an empty map
/// would make every comparison below trivially satisfied — the classic
/// self-passing shape.
fn parse_declaration(doc: &str) -> Declaration {
    let start = doc.find(BEGIN).expect(
        "docs/delivery-semantics.md has lost its DELIVERY-SEMANTICS-MACHINE-READABLE block",
    );
    let rest = &doc[start + BEGIN.len()..];
    let end = rest
        .find(END)
        .expect("DELIVERY-SEMANTICS-MACHINE-READABLE block is not terminated");

    let mut out = Declaration::default();
    for line in rest[..end].lines() {
        let line = line.trim();
        if line.is_empty() || !line.contains('=') {
            continue;
        }
        // Prose inside the block explains the vocabulary; it must not be parsed
        // as data. Only `key = value` where key is a bare platform (or
        // `platform.cap`) counts, so a sentence containing '=' is skipped
        // rather than becoming a phantom row.
        let (k, v) = line.split_once('=').expect("checked above");
        let (k, v) = (k.trim().to_string(), v.trim().to_string());
        if k.contains(char::is_whitespace) {
            continue;
        }

        // `.cap_measured` first: it is the longer suffix, and a `<platform>.cap_measured`
        // line falling through to the guarantee arm below would panic on an unknown
        // guarantee word rather than being read as the verdict it is.
        if let Some(platform) = k.strip_suffix(".cap_measured") {
            assert!(
                matches!(v.as_str(), "no" | "live"),
                "unknown cap_measured verdict {v:?} for {k:?} -- the vocabulary is no / live. \
                 `live` means a boundary probe ran against the real platform; there is no \
                 third state, because \"we are fairly confident\" is what this file exists \
                 to stop being written down"
            );
            assert!(
                out.cap_measured
                    .insert(platform.to_string(), v == "live")
                    .is_none(),
                "{k:?} is declared twice"
            );
            continue;
        }

        // `.cap_source` before `.cap` for the same reason `.cap_measured` is
        // first: neither ends in `.cap`, so an unhandled one falls through to
        // the guarantee arm and panics on an unknown guarantee word.
        if let Some(platform) = k.strip_suffix(".cap_source") {
            assert!(
                v.starts_with("https://"),
                "{k:?} must be a URL a reader can open, got {v:?}. A prose assurance here \
                 would be the same defect the source column exists to close: a number \
                 vouching for itself"
            );
            assert!(
                out.cap_source
                    .insert(platform.to_string(), v.clone())
                    .is_none(),
                "{k:?} is declared twice"
            );
            continue;
        }

        if let Some(platform) = k.strip_suffix(".cap") {
            let n: usize = v.parse().unwrap_or_else(|_| {
                panic!("{k:?} must be a plain char count in decimal, got {v:?}")
            });
            assert!(
                out.caps.insert(platform.to_string(), n).is_none(),
                "{k:?} is declared twice"
            );
            continue;
        }

        assert!(
            matches!(
                v.as_str(),
                "exactly-once" | "exactly-once-below-cap" | "at-most-once" | "at-least-once"
            ),
            "unknown guarantee {v:?} for {k:?} — the vocabulary is exactly-once / \
             exactly-once-below-cap / at-most-once / at-least-once"
        );
        assert!(
            out.guarantees.insert(k.clone(), v).is_none(),
            "{k:?} is declared twice"
        );
    }
    assert!(
        !out.guarantees.is_empty(),
        "parsed zero rows out of the declaration — the parser or the document is broken, and \
         an empty table would make this whole test vacuous"
    );
    out
}

/// The machine-readable block, parsed.
///
/// `caps` is separate from `guarantees` because a cap is a fact about the
/// adapter rather than one of the guarantee's values, and the cross-checks
/// between the maps are the point (see [`disagreements`]).
///
/// **Generalised 2026-08-26 (wayland#934).** Until then a `.cap` row meant "the
/// boundary of a conditional guarantee" and was legal only on the one
/// `exactly-once-below-cap` row; every other adapter's cap was checked by an
/// `assert_eq!` against the literal its own function returns one line above.
/// A cap row now means `max_message_len()`, for every adapter that declares
/// one, and the old arm rejecting a cap row on an unconditional guarantee is
/// gone rather than merely relaxed.
#[derive(Default, Clone)]
struct Declaration {
    guarantees: BTreeMap<String, String>,
    caps: BTreeMap<String, usize>,
    /// `platform -> was the cap measured against the real platform`. Required beside every
    /// cap row. `false` for all seven today, and that is the point: an `assert_eq!` against
    /// a number implies the number was verified, and until a boundary probe runs, nothing
    /// has verified it against anything but our own adapter.
    cap_measured: BTreeMap<String, bool>,
    /// `platform -> the vendor page the cap is derived from`. Required beside every cap row.
    ///
    /// Added 2026-08-28 (wayland#934). `cap_measured` says whether the number was checked
    /// against the PLATFORM; this says what the number was READ FROM in the first place, which
    /// is a different question and the one that was never asked. Reading these pages found two
    /// of the seven caps wrong — `msteams` taken from the Incoming Webhook surface and misread
    /// from KB into characters, `matrix` computed at two UTF-8 bytes per character where UTF-8
    /// uses four — and neither is drift a cap-vs-adapter comparison could see, because in both
    /// cases the document and the adapter agreed with each other perfectly.
    cap_source: BTreeMap<String, String>,
}

/// What an adapter, as the production factory builds it, actually reports.
///
/// `max_message_len` joined `supports` here on 2026-07-31. The exactly-once
/// claim is conditional on the cap (see `docs/delivery-semantics.md` §4.1), and
/// **Matrix's cap — the one number the whole surviving claim rests on — had no
/// test of any kind.** The other six caps were each covered by an `assert_eq!`
/// against the literal the function returns, which checks the constant against
/// itself; this binds the number to the document instead.
#[derive(Clone, Copy, Debug)]
struct Measured {
    supports: bool,
    max_message_len: Option<usize>,
}

/// Build every constructible adapter and read its declared capability.
fn measured_capabilities() -> BTreeMap<String, Measured> {
    let mut out = BTreeMap::new();
    for platform in constructible_platforms() {
        let factory = channel_factory_for(platform)
            .unwrap_or_else(|| panic!("registry has no factory for {platform:?}"));
        let channel = factory(
            format!("fixture-{platform}"),
            &fixture_options(platform),
            creds(),
        )
        .unwrap_or_else(|e| panic!("could not construct {platform:?} from its fixture: {e}"));
        out.insert(
            platform.to_string(),
            Measured {
                supports: channel.supports_outbound_idempotency(),
                max_message_len: channel.max_message_len(),
            },
        );
    }
    out
}

/// The comparator, factored out so the failure-direction tests can drive the
/// SAME code the passing test drives. A second, parallel implementation used
/// only by the negative tests would prove nothing about this one.
///
/// Returns one human-readable line per disagreement; empty means agreement.
fn disagreements(declared: &Declaration, measured: &BTreeMap<String, Measured>) -> Vec<String> {
    let mut out = Vec::new();

    for (platform, m) in measured {
        let supports = m.supports;
        match declared.guarantees.get(platform) {
            None => out.push(format!(
                "{platform}: constructible by the registry but has NO row in \
                 docs/delivery-semantics.md"
            )),
            Some(guarantee) => {
                // Both exactly-once flavours mean the adapter transmits a key.
                // The flavours differ in WHEN it rides, not in whether the
                // capability bit is set.
                let expected_supports = guarantee.starts_with("exactly-once");
                if expected_supports != supports {
                    out.push(format!(
                        "{platform}: the document says {guarantee:?} (implying \
                         supports_outbound_idempotency() == {expected_supports}) but the adapter \
                         returns {supports}"
                    ));
                }

                // The cap, for EVERY adapter. A cap row is present exactly
                // when the adapter reports one, and carries the same number.
                // This is the half that stops the six `assert_eq!`s in the
                // adapter crates from being the only thing checking a number
                // the chunker reads on every send.
                match (m.max_message_len, declared.caps.get(platform)) {
                    (Some(actual), None) => out.push(format!(
                        "{platform}: the adapter caps a single message at {actual} chars but the \
                         block carries no {platform}.cap row. send_to_keyed chunks on that \
                         number, so it is load-bearing whether or not the guarantee mentions it"
                    )),
                    (Some(actual), Some(&cap)) if actual != cap => out.push(format!(
                        "{platform}: the document says the cap is {cap} chars but the adapter's \
                         max_message_len() is {actual}"
                    )),
                    (None, Some(&cap)) => out.push(format!(
                        "{platform}: carries a {platform}.cap row ({cap}) but the adapter reports \
                         max_message_len() == None, so the row describes a cap that does not exist"
                    )),
                    _ => {}
                }

                // The guarantee-specific rules, on top of the cap agreement.
                match guarantee.as_str() {
                    // A conditional promise with its condition left unstated.
                    "exactly-once-below-cap" if !declared.caps.contains_key(platform) => {
                        out.push(format!(
                            "{platform}: declared exactly-once-below-cap but the block carries no \
                             {platform}.cap row, so the condition the guarantee depends on is \
                             unstated"
                        ))
                    }
                    // The rule that stops this document sliding back to the
                    // unconditional sentence it carried until 2026-07-31: a
                    // finite cap makes bare `exactly-once` false above it.
                    "exactly-once" => {
                        if let Some(actual) = m.max_message_len {
                            out.push(format!(
                                "{platform}: declared bare exactly-once, but the adapter caps a \
                                 single message at {actual} chars. Above that, send_to_keyed \
                                 chunks the body and transmits NO key, so the guarantee is \
                                 at-least-once there. Declare exactly-once-below-cap with a \
                                 {platform}.cap row"
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // A cap row for a platform with no guarantee row at all.
    for platform in declared.caps.keys() {
        if !declared.guarantees.contains_key(platform) {
            out.push(format!(
                "{platform}: has a {platform}.cap row but no guarantee row"
            ));
        }
        // A number with no statement about where it came from reads as verified.
        if !declared.cap_measured.contains_key(platform) {
            out.push(format!(
                "{platform}: has a {platform}.cap row but no {platform}.cap_measured verdict. \
                 A cap asserted with nothing said about its provenance reads as though the \
                 platform confirmed it, and no platform has"
            ));
        }
        // And a number nobody can trace to a vendor page is a number vouching
        // for itself, which is the whole of wayland#934.
        if !declared.cap_source.contains_key(platform) {
            out.push(format!(
                "{platform}: has a {platform}.cap row but no {platform}.cap_source. A cap with \
                 no citation cannot be checked by a reader, only agreed with by our own code — \
                 which is how {platform}'s peers msteams and matrix each carried a wrong number \
                 through a green build"
            ));
        }
    }

    // The converse: a verdict about a cap that is not declared.
    for platform in declared.cap_measured.keys() {
        if !declared.caps.contains_key(platform) {
            out.push(format!(
                "{platform}: has a {platform}.cap_measured verdict but no {platform}.cap row, so \
                 the verdict is about a number the block does not carry"
            ));
        }
    }

    // The same converse for the citation.
    for platform in declared.cap_source.keys() {
        if !declared.caps.contains_key(platform) {
            out.push(format!(
                "{platform}: has a {platform}.cap_source but no {platform}.cap row, so the \
                 citation is about a number the block does not carry"
            ));
        }
    }

    for platform in declared.guarantees.keys() {
        // iMessage is compiled out off macOS, so its row is expected to have no
        // adapter here. Every other row must name something constructible.
        if platform == "imessage" && !cfg!(target_os = "macos") {
            continue;
        }
        if !measured.contains_key(platform) {
            out.push(format!(
                "{platform}: has a row in docs/delivery-semantics.md but the registry cannot \
                 construct it"
            ));
        }
    }

    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Direction 1 — CAN IT PASS. The real document against the real adapters.
// ---------------------------------------------------------------------------

#[test]
fn declaration_matches_every_adapter() {
    let declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    // Guard against a vacuous run: if the fixture set silently shrank, the
    // comparison could go green having checked almost nothing.
    let expected_rows = if cfg!(target_os = "macos") { 10 } else { 9 };
    assert_eq!(
        measured.len(),
        expected_rows,
        "expected {expected_rows} constructible adapters on this platform, built {}: {:?}",
        measured.len(),
        measured.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        declared.guarantees.len(),
        10,
        "docs/delivery-semantics.md must carry a row for all ten adapters (including the \
         macOS-only iMessage), found {}",
        declared.guarantees.len()
    );
    // Seven adapters declare a finite cap; three inherit `None` from the trait
    // default. Zero cap rows -- or a shrinking count -- would mean the parser
    // had silently stopped seeing `<platform>.cap` lines, and every cap check
    // below would then be vacuously satisfied.
    assert_eq!(
        declared.caps.len(),
        7,
        "expected seven <platform>.cap rows (slack, matrix, discord, telegram, sms, whatsapp, \
         msteams -- email, signal and iMessage report None). Found {}: {:?}",
        declared.caps.len(),
        declared.caps
    );
    assert_eq!(
        declared.cap_source.len(),
        declared.caps.len(),
        "every cap row needs a cap_source beside it; got {} caps and {} sources",
        declared.caps.len(),
        declared.cap_source.len()
    );
    assert_eq!(
        declared.cap_measured.len(),
        declared.caps.len(),
        "every cap row needs a cap_measured verdict beside it; got {} caps and {} verdicts",
        declared.caps.len(),
        declared.cap_measured.len()
    );

    let problems = disagreements(&declared, &measured);
    assert!(
        problems.is_empty(),
        "docs/delivery-semantics.md has drifted from the adapters:\n  {}",
        problems.join("\n  ")
    );
}

/// The ratio itself is the headline of the document and of criterion 24-C1.
/// Asserting it separately means a change to it fails with a message that says
/// what actually changed, rather than as one line inside a diff.
///
/// **Was `exactly_three` until 2026-07-30.** Discord was removed after a live
/// replay at the real platform produced a duplicate: it accepts the `nonce` and
/// echoes it back but never deduplicates on it, at any delay including zero.
/// See `docs/delivery-semantics.md` §8. The lesson this assertion should carry
/// forward is that Discord was in this list on the strength of a mockito test,
/// which can only prove what we SEND — never what the platform HONOURS.
#[test]
fn exactly_one_adapter_is_exactly_once() {
    let measured = measured_capabilities();
    let mut idempotent: Vec<&str> = measured
        .iter()
        .filter(|(_, m)| m.supports)
        .map(|(k, _)| k.as_str())
        .collect();
    idempotent.sort_unstable();

    assert_eq!(
        idempotent,
        vec!["matrix"],
        "the set of exactly-once adapters changed. This is a customer-facing guarantee: update \
         docs/delivery-semantics.md (both the table and the machine-readable block) in the same \
         commit, and PROVE AT THE REAL PLATFORM that it honours a replayed key before adding \
         one. On 2026-07-30 both Discord and Slack were driven at their real APIs for the first \
         time and BOTH produced TWO messages from a replayed key. Each had been in this list on \
         the strength of a mockito test. A mock proves what we send; it cannot prove what the \
         destination does with it, and for these two it was wrong. Matrix is the only member \
         that has survived a live replay — the shipped binary was killed mid-send against \
         matrix.org and the homeserver returned the original event_id."
    );
}

// ---------------------------------------------------------------------------
// Direction 2 — CAN IT FAIL. Same comparator, mutated inputs, executed here.
// ---------------------------------------------------------------------------

#[test]
fn comparator_rejects_a_flipped_row() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    // Precondition: green before the mutation, so the red below is caused by
    // the mutation and not by pre-existing drift.
    assert!(
        disagreements(&declared, &measured).is_empty(),
        "the unmutated comparison must be green for this test to mean anything"
    );

    // Claim exactly-once for an adapter that provides no such thing. This is
    // the dangerous direction: a document over-promising relative to the code.
    declared
        .guarantees
        .insert("telegram".into(), "exactly-once".into());

    // TWO disagreements since 2026-07-31, and both are real: Telegram neither
    // transmits a key NOR is uncapped, so the mutated row is false in two
    // independent ways. Asserting `len() == 1` here would have been the weaker
    // claim — the count is pinned so a rule silently ceasing to fire still
    // reddens.
    let problems = disagreements(&declared, &measured);
    assert_eq!(
        problems.len(),
        2,
        "expected the capability mismatch AND the bare-exactly-once-over-a-capped-adapter \
         report, got: {problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("telegram:") && p.contains("returns false")),
        "the capability disagreement must name telegram and the measured value: {problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("telegram:") && p.contains("declared bare exactly-once")),
        "the unconditional claim over a 4096-char cap must also be reported: {problems:?}"
    );
}

#[test]
fn comparator_rejects_a_downgraded_row() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    // The other direction: the code gained a guarantee the document has not
    // caught up with. Less dangerous, still drift.
    //
    // Keyed on matrix since 2026-07-30 — this used slack, which was an
    // exactly-once adapter until a live replay showed Slack ignoring the key.
    // The mutation has to name an adapter that really does declare `true`, or
    // the "document downgrades a real guarantee" case is not being exercised.
    declared
        .guarantees
        .insert("matrix".into(), "at-most-once".into());

    // Back to ONE disagreement since 2026-08-26. It was two between 07-31 and
    // then, because a leftover `matrix.cap` row under an unconditional
    // guarantee was itself drift. Under the generalised meaning a cap row is a
    // fact about the adapter, so it is *correct* to keep carrying it here —
    // Matrix really does cap at 16,384 whatever guarantee the row claims. The
    // count is pinned so this reduction is a deliberate consequence of the
    // wayland#934 change rather than a rule quietly ceasing to fire.
    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("matrix:") && p.contains("returns true")),
        "got: {problems:?}"
    );
}

#[test]
fn comparator_rejects_a_missing_row() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    // A new adapter shipped with no row is the drift a row-by-row comparison
    // cannot see on its own — there is no row to disagree with.
    declared.guarantees.remove("matrix");
    declared.caps.remove("matrix");
    declared.cap_measured.remove("matrix");
    declared.cap_source.remove("matrix");

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("NO row in"),
        "the missing row must be reported as missing, not silently skipped: {problems:?}"
    );
}

#[test]
fn comparator_rejects_a_row_for_an_adapter_that_does_not_exist() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    declared
        .guarantees
        .insert("carrierpigeon".into(), "exactly-once".into());

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("cannot construct it"),
        "got: {problems:?}"
    );
}

// ---------------------------------------------------------------------------
// Direction 2b — the CAP half of a conditional guarantee, both directions.
//
// Added 2026-07-31 with §4.1. Matrix's `max_message_len` is the single number
// the surviving exactly-once claim is conditional on and it had NO test of any
// kind; the six caps that were "covered" were each covered by an `assert_eq!`
// against the literal their own function returns, which cannot fail for any
// reason a reader cares about.
// ---------------------------------------------------------------------------

/// Can it pass: every declared cap against the adapter that declares it.
///
/// **Was Matrix-only until 2026-08-26 (wayland#934).** The other six caps were each covered
/// by an `assert_eq!` in their own adapter crate against the literal the function returns one
/// line above, which restates the code and would keep passing if the number were wrong. This
/// binds all seven to an independent artifact.
#[test]
fn every_declared_cap_is_the_adapters_real_cap() {
    let declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    // Not vacuous, and pinned: a shrinking set would make the loop below check less while
    // still reporting green.
    let mut platforms: Vec<&str> = declared.caps.keys().map(String::as_str).collect();
    platforms.sort_unstable();
    assert_eq!(
        platforms,
        vec![
            "discord", "matrix", "msteams", "slack", "sms", "telegram", "whatsapp"
        ],
        "the set of capped adapters changed. Email, Signal and iMessage inherit the trait \
         default of None and must NOT gain a cap row; anything else that reports Some(n) must."
    );

    for (platform, &cap) in &declared.caps {
        let actual = measured[platform].max_message_len;
        assert_eq!(
            actual,
            Some(cap),
            "docs/delivery-semantics.md §4.2 says {platform} caps a single message at {cap} \
             chars, but the adapter the production factory builds reports \
             max_message_len() == {actual:?}. One of the two is wrong, and the document is \
             the customer-facing one."
        );
    }
}

/// **No cap has been measured at its real platform, and the file has to say so.**
///
/// This is the half of wayland#934 that the generalisation above does NOT close. Comparing
/// the document's number to the adapter's number is a drift check; both numbers are ours.
/// The `cap_measured` verdict exists so that fact is stated rather than left to be inferred
/// from an `assert_eq!` that looks like verification.
///
/// If a boundary probe ever runs and this assertion fails, that is the intended way to find
/// out: flip the row to `live`, record the run in §4.2, and narrow this test to the rows that
/// are still `no`.
#[test]
fn no_cap_is_claimed_measured_at_a_real_platform_yet() {
    let declared = parse_declaration(DECLARATION);
    assert!(
        !declared.cap_measured.is_empty(),
        "parsed zero cap_measured verdicts, which would make the claim below vacuous"
    );
    let claimed: Vec<&str> = declared
        .cap_measured
        .iter()
        .filter(|&(_, &live)| live)
        .map(|(k, _)| k.as_str())
        .collect();
    // wayland#934: slack and discord WERE boundary-probed at the real platform on
    // 2026-08-27 (slack 4,040 intact / 4,041 splits; discord 2,000 ok / 2,001 refused
    // 400 50035). The guard still holds the remaining five to the same bar, so it keeps
    // its teeth: it reddens the moment a sixth platform claims `live` without evidence.
    //
    // Those two numbers are no longer only a date in a comment. `tests/
    // live_message_cap_boundary.rs` carries them as a committed cell per platform,
    // checks each against the cap the production factory builds, and re-derives it
    // when the live cell is run. That file also enforces the other half of this
    // exemption from the opposite side: a `cap_measured = live` row with no measured
    // cell there fails, and a measured cell with no `live` row fails too. Neither
    // direction of a half-updated commit can survive both files.
    let unproven: Vec<&str> = claimed
        .iter()
        .copied()
        .filter(|p| !matches!(*p, "slack" | "discord"))
        .collect();
    assert!(
        unproven.is_empty(),
        "{unproven:?} claim cap_measured = live. That word may only be written after a boundary \
         probe has sent a body of exactly `cap` chars and one of `cap + 1` at the REAL \
         destination and read what arrived. §4.2 names the credential each probe is waiting \
         on; if one of them ran, record it there in the same commit."
    );
}

/// Can it fail, 1: the document's number drifts from the adapter's.
#[test]
fn comparator_rejects_a_cap_that_does_not_match_the_adapter() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();
    assert!(
        disagreements(&declared, &measured).is_empty(),
        "the unmutated comparison must be green for this test to mean anything"
    );

    declared.caps.insert("matrix".into(), 16_383);

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("16383") && problems[0].contains("16384"),
        "the disagreement must name both the documented and the real cap: {problems:?}"
    );
}

/// Can it fail, 2: a conditional guarantee with the condition left unstated.
#[test]
fn comparator_rejects_a_conditional_row_with_no_cap() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    declared.caps.remove("matrix");
    declared.cap_measured.remove("matrix");
    declared.cap_source.remove("matrix");

    // Two since 2026-08-26, and both are real: the guarantee has lost the condition it
    // depends on, AND a capped adapter has lost its cap row. Pinning the count means a rule
    // silently ceasing to fire still reddens.
    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 2, "got: {problems:?}");
    assert!(
        problems.iter().any(|p| p.contains("no matrix.cap row")),
        "the unstated condition must be reported: {problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("caps a single message at 16384 chars but the block carries no")),
        "the missing cap row must be reported in its own right: {problems:?}"
    );
}

/// Can it fail, 3 — **the important one.**
///
/// This is the exact state the document was in until 2026-07-31: a bare
/// `exactly-once` claim over an adapter that caps a single message, so the
/// promise is false above the cap. No row in the real document exercises this
/// rule any more, which is precisely why it needs a test that constructs the
/// state: an unexercised rule is indistinguishable from an absent one.
#[test]
fn comparator_rejects_bare_exactly_once_over_a_capped_adapter() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    // Regress the document to its pre-2026-07-31 wording.
    declared
        .guarantees
        .insert("matrix".into(), "exactly-once".into());
    declared.caps.remove("matrix");
    declared.cap_measured.remove("matrix");
    declared.cap_source.remove("matrix");

    let problems = disagreements(&declared, &measured);
    assert_eq!(
        problems.len(),
        2,
        "the old, false, unconditional wording must be rejected: {problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("declared bare exactly-once") && p.contains("16384")),
        "the disagreement must say why the unconditional claim is false and name the cap: \
         {problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.contains("but the block carries no matrix.cap row")),
        "dropping the cap row is separately wrong now that every capped adapter needs one: \
         {problems:?}"
    );
}

/// Can it fail, 4: a cap row for an adapter that has no cap.
///
/// **This replaces `comparator_rejects_a_cap_row_on_an_unconditional_guarantee`**, which
/// asserted that `telegram.cap = 4096` was drift. Under the generalised meaning (wayland#934)
/// that row is not drift, it is required — Telegram really does cap at 4096 — so the old test
/// would have been asserting the opposite of the new rule. The rule that survives is the one
/// that was doing the work: a cap row has to describe a cap the adapter actually reports.
#[test]
fn comparator_rejects_a_cap_row_for_an_uncapped_adapter() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();
    assert!(
        disagreements(&declared, &measured).is_empty(),
        "the unmutated comparison must be green for this test to mean anything"
    );

    // Email inherits the trait default and reports None.
    declared.caps.insert("email".into(), 1000);
    declared.cap_measured.insert("email".into(), false);
    declared
        .cap_source
        .insert("email".into(), "https://example.invalid/rfc5321".into());

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("max_message_len() == None"),
        "got: {problems:?}"
    );
}

/// Can it fail, 5 — **the rule wayland#934 added.** A capped adapter with no cap row.
///
/// This is the state six of the seven adapters were in until 2026-08-26: a real cap, read by
/// `send_to_keyed` on every send, and nothing outside its own function checking the number.
#[test]
fn comparator_rejects_a_capped_adapter_with_no_cap_row() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    declared.caps.remove("slack");
    declared.cap_measured.remove("slack");
    declared.cap_source.remove("slack");

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("caps a single message at 4000 chars")
            && problems[0].contains("no slack.cap row"),
        "the disagreement must name the platform and the unstated number: {problems:?}"
    );
}

/// Can it fail, 6: a cap asserted with nothing said about where it came from.
#[test]
fn comparator_rejects_a_cap_row_with_no_measured_verdict() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    declared.cap_measured.remove("slack");

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("no slack.cap_measured verdict"),
        "got: {problems:?}"
    );
}

/// Can it fail, 8 — **the rule wayland#934 added on 2026-08-28.** A cap with no citation.
///
/// The state every row was in until that date: a number agreed on by our document and our
/// adapter and traceable to nothing outside the programme. Two of the seven were wrong, and
/// both passed every check in this file, because both halves of every comparison were ours.
#[test]
fn comparator_rejects_a_cap_row_with_no_source() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();
    assert!(
        disagreements(&declared, &measured).is_empty(),
        "the unmutated comparison must be green for this test to mean anything"
    );

    declared.cap_source.remove("msteams");

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("no msteams.cap_source"),
        "the missing citation must be named: {problems:?}"
    );
}

/// Can it fail, 9: a citation about a cap the block does not carry.
#[test]
fn comparator_rejects_a_source_with_no_cap_row() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    declared
        .cap_source
        .insert("signal".into(), "https://example.invalid/signal-cli".into());

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("no signal.cap row"),
        "got: {problems:?}"
    );
}

/// Every cap citation must be a URL, and a bare assurance must be refused.
///
/// The parser enforces this, so the check is on the parser: feed it a block whose source is a
/// sentence rather than a link and require the panic. Without this arm the `https://` rule
/// would be an unexercised branch, and an unexercised rule is indistinguishable from an
/// absent one.
#[test]
#[should_panic(expected = "must be a URL a reader can open")]
fn the_parser_refuses_a_cap_source_that_is_not_a_link() {
    let doc = format!(
        "{BEGIN}\nslack = at-most-once\nslack.cap = 4000\nslack.cap_measured = live\n\
         slack.cap_source = we checked with the team\n{END}"
    );
    let _ = parse_declaration(&doc);
}

/// Can it fail, 7: a verdict about a cap the block does not carry.
#[test]
fn comparator_rejects_a_measured_verdict_with_no_cap_row() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    declared.cap_measured.insert("email".into(), false);

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("no email.cap row"),
        "got: {problems:?}"
    );
}

/// The parser must not turn the explanatory prose inside the block into rows.
///
/// The block gained several sentences with the vocabulary in them on
/// 2026-07-31. A parser that swallowed those would either panic on an unknown
/// guarantee or, worse, invent platforms — and the row-count assertions
/// elsewhere would then be measuring the prose.
#[test]
fn the_parser_ignores_the_prose_inside_the_block() {
    let declared = parse_declaration(DECLARATION);
    let mut names: Vec<&str> = declared.guarantees.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "discord", "email", "imessage", "matrix", "msteams", "signal", "slack", "sms",
            "telegram", "whatsapp"
        ],
        "the parser picked up something that is not an adapter row"
    );
}

/// The prose table and the machine-readable block are two statements of the
/// same fact, and the whole point of the block is that it is cheap to parse —
/// which makes it cheap to update *alone*, leaving the human-readable table
/// stale. That would satisfy every test above while lying to the reader.
#[test]
fn the_prose_table_agrees_with_the_machine_readable_block() {
    let declared = parse_declaration(DECLARATION);

    // The row labels as they appear in the prose table's first column.
    let prose_label = |platform: &str| -> &'static str {
        match platform {
            "slack" => "**Slack**",
            "matrix" => "**Matrix**",
            "discord" => "**Discord**",
            "telegram" => "**Telegram**",
            "sms" => "**Twilio SMS**",
            "whatsapp" => "**WhatsApp**",
            "email" => "**Email**",
            "signal" => "**Signal**",
            "imessage" => "**iMessage**",
            "msteams" => "**MS Teams**",
            other => panic!("no prose label known for {other:?}"),
        }
    };

    for (platform, guarantee) in &declared.guarantees {
        let label = prose_label(platform);
        let row = DECLARATION
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("| {label}")))
            .unwrap_or_else(|| panic!("no prose table row starting with {label} for {platform:?}"));

        // What the prose row must, and must not, say for each machine label.
        //
        // The conditional row is the interesting case: it is REQUIRED to state
        // both halves. A row that said only "exactly-once" while the block said
        // `exactly-once-below-cap` would be precisely the omission this whole
        // change exists to close — the reader would take away the unconditional
        // promise the document carried until 2026-07-31.
        let (must_have, must_not_have): (&[&str], &[&str]) = match guarantee.as_str() {
            "exactly-once" => (&["exactly-once"], &["at-most-once", "at-least-once"]),
            "exactly-once-below-cap" => (&["exactly-once", "at-least-once"], &["at-most-once"]),
            "at-most-once" => (&["at-most-once"], &["exactly-once"]),
            "at-least-once" => (&["at-least-once"], &["exactly-once", "at-most-once"]),
            other => panic!("no prose expectation defined for guarantee {other:?}"),
        };

        for needle in must_have {
            assert!(
                row.contains(needle),
                "prose row for {platform:?} is declared {guarantee:?} but does not say \
                 {needle:?}:\n  {row}"
            );
        }
        for needle in must_not_have {
            assert!(
                !row.contains(needle),
                "prose row for {platform:?} is declared {guarantee:?} but also says {needle:?}, \
                 so it is ambiguous:\n  {row}"
            );
        }

        // For a conditional row the prose must also carry the actual number.
        // "exactly-once below a cap" without saying which cap is a caveat a
        // reader cannot act on.
        if guarantee == "exactly-once-below-cap" {
            let cap = declared.caps.get(platform).unwrap_or_else(|| {
                panic!("{platform:?} is exactly-once-below-cap but has no cap row")
            });
            // Rendered with a thousands separator in the prose table.
            let plain = cap.to_string();
            let grouped = format!(
                "{},{}",
                &plain[..plain.len() - 3],
                &plain[plain.len() - 3..]
            );
            assert!(
                row.contains(&plain) || row.contains(&grouped),
                "prose row for {platform:?} must state the cap ({plain} or {grouped}) that its \
                 guarantee is conditional on:\n  {row}"
            );
        }
    }
}

/// §5 must keep carrying the measurement, and must not go back to the
/// interpretation the measurement refuted.
///
/// # Why this test changed shape
///
/// It used to read `the_windows_duplicate_finding_is_still_disclosed`, and it
/// asserted that the document keeps saying Windows can duplicate on any adapter
/// — because `F24-GWP-H1` was believed measured, open and unfixed.
///
/// **The finding was then refuted with its own evidence.** Every repeat in that
/// run carried a DIFFERENT delivery id (5 of 5 keyed jobs, zero replays); the
/// jobs were submitted `every:15`, which `wcore-cron/src/trigger.rs:238`
/// rate-floors to sixty seconds; and the heartbeat in the same run — never
/// inside a kill window — recurred with scheduled deltas of 60068 ms and
/// 64940 ms, which nobody called duplicates. Windows crosses the period
/// reliably, not exclusively.
///
/// The old test would still have PASSED over the rewritten section, because
/// both of the strings it grepped for survive the correction. A gate that keeps
/// passing after the claim beneath it has been inverted is not measuring the
/// claim; so it is rewritten here rather than left to go on being green.
#[test]
fn the_recurrence_section_keeps_its_measurement_and_its_correction() {
    // The evidence. A warning — or a correction — without the numbers it rests
    // on is an assertion.
    for evidence in [
        "{2: 12, 3: 1}",     // the measured Windows arrival histogram
        "60068",             // the heartbeat delta that measures the 60s floor
        "trigger.rs:238",    // where the floor is applied
        "5 of 5 keyed jobs", // the delivery-id result that refuted the finding
    ] {
        assert!(
            DECLARATION.contains(evidence),
            "docs/delivery-semantics.md §5 has lost {evidence:?}, which is part of the \
             measurement the section rests on"
        );
    }
    // The finding is still NAMED, so a reader who arrives with the id can find
    // out what became of it. Retiring a finding silently is how a refutation
    // becomes indistinguishable from an oversight.
    assert!(
        DECLARATION.contains("F24-GWP-H1"),
        "docs/delivery-semantics.md no longer names F24-GWP-H1. The finding was refuted, not \
         forgotten, and the id has to remain findable."
    );

    // And the refuted sentence must not come back. A negative assertion is
    // worthless on a dead instrument (LANE-BRIEF §3b-i), so the known-positive
    // is checked in the same breath: the phrase IS present in the document, as
    // the quoted description of what the section used to say.
    let refuted = "re-fires cron jobs that have already fired";
    assert!(
        DECLARATION.contains(refuted),
        "known-positive for this search: §5 quotes the sentence it is correcting, so a search \
         that cannot find it here is a broken search rather than a clean document"
    );
    let occurrences = DECLARATION.matches(refuted).count();
    assert_eq!(
        occurrences, 1,
        "the refuted sentence appears {occurrences} times. It belongs in §5 exactly once, inside \
         the quotation of what the section previously claimed — a second occurrence means the \
         claim has been re-asserted somewhere as fact"
    );
    // The correction itself, in the section's own words.
    assert!(
        DECLARATION.contains("**Both halves of that sentence are wrong**"),
        "§5 quotes the old claim but no longer states that it is wrong — which leaves the \
         document asserting the refuted sentence"
    );
}

/// §4.2's human-readable cap table and the machine-readable block are two statements of the
/// same facts, and the block is the cheap one to update — which makes it the cheap one to
/// update *alone*, leaving the table a reader actually reads stale. That would satisfy every
/// test above while lying.
///
/// Added 2026-08-26 with the wayland#934 generalisation.
#[test]
fn the_cap_table_agrees_with_the_machine_readable_block() {
    let declared = parse_declaration(DECLARATION);

    // Scope the search to §4.2. The §2 table uses the same row labels and comes first, so an
    // unscoped `lines().find()` would silently read the wrong table and assert nothing about
    // this one.
    let start = DECLARATION
        .find("### 4.2 ")
        .expect("docs/delivery-semantics.md has lost §4.2, the per-adapter cap table");
    let rest = &DECLARATION[start..];
    let end = rest
        .find("\n## ")
        .expect("§4.2 is not terminated by a following section");
    let section = &rest[..end];

    let label = |platform: &str| -> &'static str {
        match platform {
            "slack" => "**Slack**",
            "matrix" => "**Matrix**",
            "discord" => "**Discord**",
            "telegram" => "**Telegram**",
            "sms" => "**Twilio SMS**",
            "whatsapp" => "**WhatsApp**",
            "msteams" => "**MS Teams**",
            other => panic!("§4.2 has no row label for {other:?}"),
        }
    };

    for (platform, &cap) in &declared.caps {
        let needle = format!("| {}", label(platform));
        let row = section
            .lines()
            .find(|l| l.trim_start().starts_with(&needle))
            .unwrap_or_else(|| panic!("§4.2 has no cap row for {platform:?} ({needle})"));

        // The number, as the table renders it (thousands separator).
        let plain = cap.to_string();
        let grouped = format!(
            "{},{}",
            &plain[..plain.len() - 3],
            &plain[plain.len() - 3..]
        );
        assert!(
            row.contains(&grouped) || row.contains(&plain),
            "§4.2's row for {platform:?} must state the cap ({grouped}):\n  {row}"
        );

        // And the verdict, which is the honesty half.
        let live = declared.cap_measured[platform];
        if live {
            assert!(
                !row.contains("NOT MEASURED"),
                "the block says {platform}.cap_measured = live but §4.2 still reads NOT \
                 MEASURED:\n  {row}"
            );
        } else {
            assert!(
                row.contains("NOT MEASURED"),
                "the block says {platform}.cap_measured = no, so §4.2's row must say NOT \
                 MEASURED rather than leaving the number looking verified:\n  {row}"
            );
        }
    }
}

/// The blocked half of wayland#934 has to stay a named, actionable list rather than decaying
/// into "later".
///
/// §4.2 records, per platform, the exact credential its boundary probe is waiting on. This
/// asserts the list is still there and still names every capped platform — otherwise the
/// remaining work becomes invisible the moment someone tidies the section, and an invisible
/// blocker is indistinguishable from a closed one.
#[test]
fn every_unmeasured_cap_names_the_credential_its_probe_is_waiting_on() {
    let declared = parse_declaration(DECLARATION);

    let start = DECLARATION
        .find("#### Which probe is blocked on which credential")
        .expect("§4.2 has lost the per-platform blocked-probe table");
    let rest = &DECLARATION[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    let section = &rest[..end];

    let label = |platform: &str| -> &'static str {
        match platform {
            "slack" => "**Slack**",
            "matrix" => "**Matrix**",
            "discord" => "**Discord**",
            "telegram" => "**Telegram**",
            "sms" => "**Twilio SMS**",
            "whatsapp" => "**WhatsApp**",
            "msteams" => "**MS Teams**",
            other => panic!("no blocked-probe label for {other:?}"),
        }
    };

    for platform in declared.caps.keys() {
        if declared.cap_measured[platform] {
            continue; // measured: nothing left to be blocked on.
        }
        let needle = format!("| {}", label(platform));
        let row = section
            .lines()
            .find(|l| l.trim_start().starts_with(&needle))
            .unwrap_or_else(|| {
                panic!(
                    "{platform:?} has an unmeasured cap but no row saying what its probe is \
                     waiting on. Every blocked item names its blocker or it is not tracked."
                )
            });
        // A row that says nothing about whether we hold the credential is not actionable.
        assert!(
            row.contains("**Yes**") || row.contains("**No.**") || row.contains("DEAD"),
            "the row for {platform:?} must state whether the credential is held:\n  {row}"
        );
    }
}
