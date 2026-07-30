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
fn parse_declaration(doc: &str) -> BTreeMap<String, String> {
    let start = doc.find(BEGIN).expect(
        "docs/delivery-semantics.md has lost its DELIVERY-SEMANTICS-MACHINE-READABLE block",
    );
    let rest = &doc[start + BEGIN.len()..];
    let end = rest
        .find(END)
        .expect("DELIVERY-SEMANTICS-MACHINE-READABLE block is not terminated");

    let mut out = BTreeMap::new();
    for line in rest[..end].lines() {
        let line = line.trim();
        if line.is_empty() || !line.contains('=') {
            continue;
        }
        let (k, v) = line.split_once('=').expect("checked above");
        let (k, v) = (k.trim().to_string(), v.trim().to_string());
        assert!(
            matches!(
                v.as_str(),
                "exactly-once" | "at-most-once" | "at-least-once"
            ),
            "unknown guarantee {v:?} for {k:?} — the vocabulary is exactly-once / \
             at-most-once / at-least-once"
        );
        assert!(
            out.insert(k.clone(), v).is_none(),
            "{k:?} is declared twice"
        );
    }
    assert!(
        !out.is_empty(),
        "parsed zero rows out of the declaration — the parser or the document is broken, and \
         an empty table would make this whole test vacuous"
    );
    out
}

/// Build every constructible adapter and read its declared capability.
fn measured_capabilities() -> BTreeMap<String, bool> {
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
            channel.supports_outbound_idempotency(),
        );
    }
    out
}

/// The comparator, factored out so the failure-direction tests can drive the
/// SAME code the passing test drives. A second, parallel implementation used
/// only by the negative tests would prove nothing about this one.
///
/// Returns one human-readable line per disagreement; empty means agreement.
fn disagreements(
    declared: &BTreeMap<String, String>,
    measured: &BTreeMap<String, bool>,
) -> Vec<String> {
    let mut out = Vec::new();

    for (platform, supports) in measured {
        match declared.get(platform) {
            None => out.push(format!(
                "{platform}: constructible by the registry but has NO row in \
                 docs/delivery-semantics.md"
            )),
            Some(guarantee) => {
                let expected_supports = guarantee == "exactly-once";
                if expected_supports != *supports {
                    out.push(format!(
                        "{platform}: the document says {guarantee:?} (implying \
                         supports_outbound_idempotency() == {expected_supports}) but the adapter \
                         returns {supports}"
                    ));
                }
            }
        }
    }

    for platform in declared.keys() {
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
        declared.len(),
        10,
        "docs/delivery-semantics.md must carry a row for all ten adapters (including the \
         macOS-only iMessage), found {}",
        declared.len()
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
        .filter(|(_, v)| **v)
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
    declared.insert("telegram".into(), "exactly-once".into());

    let problems = disagreements(&declared, &measured);
    assert_eq!(
        problems.len(),
        1,
        "expected exactly one disagreement, got: {problems:?}"
    );
    assert!(
        problems[0].starts_with("telegram:") && problems[0].contains("returns false"),
        "the disagreement must name telegram and the measured value: {problems:?}"
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
    declared.insert("matrix".into(), "at-most-once".into());

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].starts_with("matrix:") && problems[0].contains("returns true"),
        "got: {problems:?}"
    );
}

#[test]
fn comparator_rejects_a_missing_row() {
    let mut declared = parse_declaration(DECLARATION);
    let measured = measured_capabilities();

    // A new adapter shipped with no row is the drift a row-by-row comparison
    // cannot see on its own — there is no row to disagree with.
    declared.remove("matrix");

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

    declared.insert("carrierpigeon".into(), "exactly-once".into());

    let problems = disagreements(&declared, &measured);
    assert_eq!(problems.len(), 1, "got: {problems:?}");
    assert!(
        problems[0].contains("cannot construct it"),
        "got: {problems:?}"
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

    for (platform, guarantee) in &declared {
        let label = prose_label(platform);
        let row = DECLARATION
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("| {label}")))
            .unwrap_or_else(|| panic!("no prose table row starting with {label} for {platform:?}"));

        // `exactly-once` is a substring of nothing else in the vocabulary, and
        // `at-most-once` likewise, so a plain containment check is sound here.
        assert!(
            row.contains(guarantee),
            "prose row for {platform:?} does not carry the declared guarantee {guarantee:?}:\n  \
             {row}"
        );
        let other = if guarantee == "exactly-once" {
            "at-most-once"
        } else {
            "exactly-once"
        };
        assert!(
            !row.contains(other),
            "prose row for {platform:?} carries BOTH guarantees, so it is ambiguous:\n  {row}"
        );
    }
}

/// The document must keep saying that Windows can duplicate on any adapter.
///
/// F24-GWP-H1 is measured, open and unfixed, and it is the one fact that makes
/// the three exactly-once rows conditional. If someone removes the warning
/// while the finding is still open, the table starts over-promising on Windows
/// without a single row changing — which every other test here would pass.
#[test]
fn the_windows_duplicate_finding_is_still_disclosed() {
    assert!(
        DECLARATION.contains("F24-GWP-H1"),
        "docs/delivery-semantics.md no longer names F24-GWP-H1. If the defect was fixed, remove \
         this test in the same commit as the fix and cite the run that proves it; if it was not, \
         the disclosure has to stay."
    );
    assert!(
        DECLARATION.contains("{2: 12, 3: 1}"),
        "the measured Windows arrival histogram has gone from the document. It is the evidence \
         the warning rests on; a warning without it is an assertion."
    );
}
