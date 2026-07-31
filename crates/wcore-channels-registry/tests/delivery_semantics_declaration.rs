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
/// `caps` is separate from `guarantees` because a cap is a *qualifier* on a
/// guarantee rather than one of its values: only `exactly-once-below-cap` rows
/// carry one, and the cross-checks between the two maps are the point (see
/// [`disagreements`]).
#[derive(Default, Clone)]
struct Declaration {
    guarantees: BTreeMap<String, String>,
    caps: BTreeMap<String, usize>,
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

                // The cap half. This is what makes the conditional row a
                // measurement rather than a caveat: the number in the document
                // has to be the number the shipped adapter reports.
                match (guarantee.as_str(), declared.caps.get(platform)) {
                    ("exactly-once-below-cap", None) => out.push(format!(
                        "{platform}: declared exactly-once-below-cap but the block carries no \
                         {platform}.cap row, so the condition the guarantee depends on is \
                         unstated"
                    )),
                    ("exactly-once-below-cap", Some(&cap)) => match m.max_message_len {
                        None => out.push(format!(
                            "{platform}: the document says the guarantee stops at {cap} chars, \
                             but the adapter reports max_message_len() == None (no cap), so the \
                             condition describes nothing"
                        )),
                        Some(actual) if actual != cap => out.push(format!(
                            "{platform}: the document says the guarantee stops at {cap} chars \
                             but the adapter's max_message_len() is {actual}"
                        )),
                        Some(_) => {}
                    },
                    // The converse, and the rule that stops this document
                    // sliding back to the unconditional sentence it carried
                    // until 2026-07-31: a finite cap makes bare `exactly-once`
                    // false above that length.
                    ("exactly-once", _) => {
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
                    _ => {
                        if let Some(&cap) = declared.caps.get(platform) {
                            out.push(format!(
                                "{platform}: carries a {platform}.cap row ({cap}) but its \
                                 guarantee is {guarantee:?}, which has no cap condition — the cap \
                                 row implies a guarantee it does not have"
                            ));
                        }
                    }
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
    // The cap rows are the conditional half of §4.1. Zero of them would mean
    // the parser had silently stopped seeing `<platform>.cap` lines, which
    // would make every cap assertion below vacuously satisfied.
    assert!(
        !declared.caps.is_empty(),
        "parsed zero <platform>.cap rows. At least Matrix declares one, so an empty cap map is \
         a broken parser rather than a document with no conditional guarantees — and it would \
         make every cap check silently pass"
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

    // Also two since 2026-07-31: downgrading the guarantee leaves the
    // `matrix.cap` row behind, and a cap row under a guarantee with no cap
    // condition is itself drift — it implies a conditional promise the row no
    // longer makes.
    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 2, "got: {problems:?}");
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("matrix:") && p.contains("returns true")),
        "got: {problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("matrix:") && p.contains("has no cap condition")),
        "the orphaned cap row must also be reported: {problems:?}"
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

/// Can it pass: the real document's cap against the real adapter.
#[test]
fn the_declared_cap_is_the_adapters_real_cap() {
    let declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    // Not vacuous: there is at least one conditional row, and it is Matrix.
    assert_eq!(
        declared.caps.keys().collect::<Vec<_>>(),
        vec!["matrix"],
        "the set of cap-conditional rows changed"
    );
    let cap = declared.caps["matrix"];
    let actual = measured["matrix"].max_message_len;
    assert_eq!(
        actual,
        Some(cap),
        "docs/delivery-semantics.md §4.1 says the Matrix guarantee stops at {cap} chars, but the \
         adapter the production factory builds reports max_message_len() == {actual:?}. One of \
         the two is wrong, and the document is the customer-facing one."
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

    declared.caps.insert("matrix".into(), 32_767);

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("32767") && problems[0].contains("32768"),
        "the disagreement must name both the documented and the real cap: {problems:?}"
    );
}

/// Can it fail, 2: a conditional guarantee with the condition left unstated.
#[test]
fn comparator_rejects_a_conditional_row_with_no_cap() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    declared.caps.remove("matrix");

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("no matrix.cap row"),
        "got: {problems:?}"
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

    let problems = disagreements(&declared, &measured);
    assert_eq!(
        problems.len(),
        1,
        "the old, false, unconditional wording must be rejected: {problems:?}"
    );
    assert!(
        problems[0].contains("declared bare exactly-once") && problems[0].contains("32768"),
        "the disagreement must say why the unconditional claim is false and name the cap: \
         {problems:?}"
    );
}

/// Can it fail, 4: a cap row attached to a guarantee that has no cap condition,
/// which would imply a conditional promise the row does not actually make.
#[test]
fn comparator_rejects_a_cap_row_on_an_unconditional_guarantee() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    declared.caps.insert("telegram".into(), 4096);

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("has no cap condition"),
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
