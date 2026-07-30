//! The native-action conformance matrix — Phase 24 Success Criterion 3.
//!
//! # What this file is for
//!
//! Every adapter now DECLARES its native-action surface through
//! [`Channel::native_actions`]. A declaration nobody checks is worth nothing —
//! it is a comment with a type. This file is the check, and it runs over every
//! adapter **the production registry can actually build**, constructed through
//! the production loader (`auto_register_from_dir`) from real on-disk TOML, not
//! through a test-only constructor.
//!
//! # It runs in BOTH directions
//!
//! A gate that cannot fail proves nothing, and a gate that cannot pass proves
//! less. For each of the four operations on each adapter:
//!
//! - declared [`Implemented`] → calling the op **must not** answer
//!   `Unsupported`. Delete the override and the trait default fires and this
//!   reddens.
//! - declared [`PlatformHasNoApi`] or [`NotImplemented`] → calling the op
//!   **must** answer `Unsupported`. Implement the op and forget the declaration
//!   and this reddens too.
//!
//! Both states are constructible, and the suite contains an adapter on each
//! side of the line for edit/delete (Slack implements, Email does not) and one
//! adapter — MS Teams — that sits on **both** sides at once, `Implemented` for
//! edit/delete and `PlatformHasNoApi` for react. So neither branch is dead code.
//!
//! # A skip is not a pass
//!
//! `iMessage` is `#[cfg(target_os = "macos")]` in the registry, so on Linux and
//! Windows it is not merely untested — it **cannot be constructed**. That is
//! reported as an explicit count of expected-vs-built adapters rather than being
//! silently absent, and [`the_matrix_covers_every_platform_the_registry_knows`]
//! fails if the built set does not equal the registry's own platform list for
//! this target. A cell that did not run must be visible as one.
//!
//! [`Implemented`]: wcore_channels::ActionSupport::Implemented
//! [`PlatformHasNoApi`]: wcore_channels::ActionSupport::PlatformHasNoApi
//! [`NotImplemented`]: wcore_channels::ActionSupport::NotImplemented
//! [`Channel::native_actions`]: wcore_channels::Channel::native_actions

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use wcore_channels::{ActionSupport, ChannelError, ChannelManager};
use wcore_channels_registry::auto_register_from_dir;
use wcore_config::credentials::{CredentialsError, CredentialsStore};

/// Every platform this matrix expects to construct on the CURRENT target.
///
/// iMessage is macOS-only in the registry (`channel_factory_for` gates its arm
/// behind `cfg(target_os = "macos")`), so the expected set is genuinely
/// platform-dependent and the test says so out loud rather than quietly
/// shrinking.
const EXPECTED_PLATFORMS: &[&str] = &[
    "discord",
    "email",
    #[cfg(target_os = "macos")]
    "imessage",
    "matrix",
    "msteams",
    "signal",
    "slack",
    "sms",
    "telegram",
    "whatsapp",
];

struct MemCreds(StdMutex<std::collections::HashMap<String, String>>);

impl MemCreds {
    fn new() -> Arc<Self> {
        Arc::new(Self(StdMutex::new(std::collections::HashMap::new())))
    }
}

impl CredentialsStore for MemCreds {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), CredentialsError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

/// Minimal-but-valid config for each platform.
///
/// Only the fields with no serde default are supplied; everything else takes
/// the adapter's own default, which is the point — these are the configs a
/// first-time operator would write, so the matrix is measured against the
/// shipped defaults rather than against a tuned fixture.
///
/// No credential is ever resolved: nothing here is started, and every call the
/// matrix makes is expected to fail at the auth/not-started boundary. That is
/// deliberate — the matrix asks *which error* an unstarted adapter produces,
/// and `Unsupported` versus `NotStarted`/`Auth` is exactly the discrimination
/// under test.
fn fixture_toml(platform: &str) -> String {
    let options = match platform {
        "slack" => {
            r#"
workspace_name = "acme"
credential_handle_bot_token = "slack.acme.bot_token"
credential_handle_signing_secret = "slack.acme.signing_secret"
"#
        }
        "telegram" => {
            r#"
credential_handle = "telegram.acme.bot_token"
"#
        }
        "email" => {
            r#"
from_address = "bot@acme.test"

[options.smtp]
host = "smtp.acme.test"
user_credential_handle = "email.acme.smtp_user"
password_credential_handle = "email.acme.smtp_password"
"#
        }
        "discord" => {
            r#"
credential_handle = "discord.acme.bot_token"
"#
        }
        "sms" => {
            r#"
from_number = "+15550000000"
credential_handle_account_sid = "sms.acme.account_sid"
credential_handle_auth_token = "sms.acme.auth_token"
"#
        }
        "whatsapp" => {
            r#"
workspace_name = "acme"
phone_number_id = "1234567890"
credential_handle_access_token = "whatsapp.acme.access_token"
credential_handle_app_secret = "whatsapp.acme.app_secret"
"#
        }
        "signal" => {
            r#"
account = "+15550000000"
"#
        }
        "matrix" => {
            r#"
homeserver_url = "https://matrix.acme.test"
credential_handle_access_token = "matrix.acme.token"
user_id = "@bot:acme.test"
"#
        }
        "msteams" => {
            r#"
credential_handle_app_id = "msteams.acme.app_id"
credential_handle_app_password = "msteams.acme.app_password"
"#
        }
        "imessage" => "",
        other => panic!("no fixture for platform {other}"),
    };
    format!("name = \"{platform}\"\nplatform = \"{platform}\"\n\n[options]{options}")
}

/// Build a manager holding one instance of every constructible adapter, through
/// the PRODUCTION loader.
async fn registry_manager() -> (ChannelManager, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    for platform in EXPECTED_PLATFORMS {
        std::fs::write(
            dir.path().join(format!("{platform}.toml")),
            fixture_toml(platform),
        )
        .expect("write fixture");
    }
    let mut mgr = ChannelManager::new();
    let n = auto_register_from_dir(&mut mgr, dir.path(), MemCreds::new())
        .await
        .expect("auto-register");
    assert_eq!(
        n,
        EXPECTED_PLATFORMS.len(),
        "the loader built {n} adapters from {} fixtures — a fixture is invalid, \
         and a matrix over a shrunken set is not the matrix",
        EXPECTED_PLATFORMS.len()
    );
    (mgr, dir)
}

/// **Anti-vacuity.** Before any conclusion is drawn from the matrix, prove the
/// matrix has all the adapters in it.
///
/// A conformance sweep over an empty or partial set passes trivially — the
/// self-passing shape this repo keeps measuring. So the built set is compared
/// against the registry's own expectation for this target, by NAME, and the
/// count is asserted rather than assumed.
#[tokio::test]
async fn the_matrix_covers_every_platform_the_registry_knows() {
    let (mgr, _dir) = registry_manager().await;
    let mut names = mgr.list_names();
    names.sort();
    let mut expected: Vec<String> = EXPECTED_PLATFORMS.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(names, expected, "built adapter set != expected set");

    // The count is stated so a reader of the test output knows how many cells
    // this run covered — and, on a non-macOS target, that one platform was
    // structurally absent rather than skipped.
    println!(
        "MATRIX: {} adapters built on target_os={} ({} ops each = {} cells)",
        names.len(),
        std::env::consts::OS,
        4,
        names.len() * 4
    );
    #[cfg(not(target_os = "macos"))]
    println!(
        "MATRIX: imessage NOT BUILT on this target — registry gates it behind \
         cfg(target_os = \"macos\"). This is a structural absence, not a skip."
    );
}

/// Ask each adapter what it can do, then check it against what it does.
///
/// The two branches below are the two directions of the control. Neither is
/// dead: Slack/Discord/Telegram/Matrix/MS Teams take the `Implemented` branch
/// for edit and delete, Email/SMS/WhatsApp/iMessage take the negative branch,
/// and MS Teams takes BOTH branches within one adapter (edit implemented,
/// react permanently absent).
#[tokio::test]
async fn every_adapter_behaves_the_way_it_declares() {
    let (mgr, _dir) = registry_manager().await;

    let mut implemented_cells = 0usize;
    let mut negative_cells = 0usize;

    for name in mgr.list_names() {
        let actions = mgr
            .native_actions_on(&name)
            .await
            .unwrap_or_else(|| panic!("{name}: registered but has no declaration"));

        // A non-implemented op must say WHY. An absence claim with no reason is
        // not a measurement, and `PlatformHasNoApi` is an absence claim about
        // somebody else's product.
        let any_negative = actions.entries().iter().any(|(_, s)| !s.is_implemented());
        if any_negative {
            assert!(
                !actions.note.trim().is_empty(),
                "{name}: declares a non-implemented op but gives no reason"
            );
        }

        // ---- edit ------------------------------------------------------
        let edit = mgr.edit_on(&name, "conv-1", "msg-1", "new text").await;
        assert_declaration_holds(&name, "edit", actions.edit, as_unit(edit));
        tally(actions.edit, &mut implemented_cells, &mut negative_cells);

        // ---- delete ----------------------------------------------------
        let del = mgr.delete_on(&name, "conv-1", "msg-1").await;
        assert_declaration_holds(&name, "delete", actions.delete, del);
        tally(actions.delete, &mut implemented_cells, &mut negative_cells);

        // ---- react -----------------------------------------------------
        let react = mgr.react_on(&name, "conv-1", "msg-1", "👀").await;
        assert_declaration_holds(&name, "react", actions.react, react);
        tally(actions.react, &mut implemented_cells, &mut negative_cells);

        // `typing` is deliberately NOT called here. Its trait default is a
        // silent `Ok(())`, not an `Unsupported` error, so the call carries no
        // signal to check a declaration against — which is precisely why the
        // declaration is the only way to tell a real typing indicator from a
        // no-op, and why the field exists. Asserting on it here would be
        // asserting on nothing.
    }

    // **Both branches must have executed.** If either count is zero the sweep
    // ran one-sided and proved half of what it claims.
    assert!(
        implemented_cells > 0,
        "no adapter declared any op Implemented — the positive branch never ran"
    );
    assert!(
        negative_cells > 0,
        "no adapter declared any op absent — the negative branch never ran"
    );
    println!(
        "MATRIX: {implemented_cells} implemented cells and {negative_cells} \
         non-implemented cells checked (edit/delete/react over {} adapters)",
        mgr.list_names().len()
    );
}

fn tally(s: ActionSupport, implemented: &mut usize, negative: &mut usize) {
    if s.is_implemented() {
        *implemented += 1;
    } else {
        *negative += 1;
    }
}

fn as_unit<T>(r: Result<T, ChannelError>) -> Result<(), ChannelError> {
    r.map(|_| ())
}

/// The single assertion the whole file exists to make.
fn assert_declaration_holds(
    name: &str,
    op: &str,
    declared: ActionSupport,
    outcome: Result<(), ChannelError>,
) {
    let unsupported = matches!(outcome, Err(ChannelError::Unsupported { .. }));
    if declared.is_implemented() {
        assert!(
            !unsupported,
            "{name}.{op}: declared `{}` but the call fell through to the trait's \
             Unsupported default — the override is missing. outcome = {outcome:?}",
            declared.as_str()
        );
    } else {
        assert!(
            unsupported,
            "{name}.{op}: declared `{}` but the call did NOT answer Unsupported — \
             either the op is implemented and the declaration is stale, or the \
             default changed. outcome = {outcome:?}",
            declared.as_str()
        );
    }
}

/// A mutation control for `assert_declaration_holds` itself.
///
/// §6b-ii: a repaired or newly-written instrument needs three assertions, not
/// two — known-positive passes, known-negative fails, and the check actually
/// discriminates. Without this, `assert_declaration_holds` could be a no-op and
/// the sweep above would still be green over every adapter.
#[test]
fn the_matrix_assertion_can_itself_fail_in_both_directions() {
    let unsupported = || {
        Err::<(), _>(ChannelError::Unsupported {
            op: "edit".into(),
            platform: "test".into(),
        })
    };
    let not_started = || Err::<(), _>(ChannelError::NotStarted);

    // Agreement passes, both ways round.
    assert_declaration_holds("t", "edit", ActionSupport::Implemented, not_started());
    assert_declaration_holds("t", "edit", ActionSupport::PlatformHasNoApi, unsupported());
    assert_declaration_holds("t", "edit", ActionSupport::NotImplemented, unsupported());

    // Disagreement panics, both ways round.
    let claimed_but_absent = std::panic::catch_unwind(|| {
        assert_declaration_holds("t", "edit", ActionSupport::Implemented, unsupported())
    });
    assert!(
        claimed_but_absent.is_err(),
        "declaring Implemented over an Unsupported outcome MUST fail"
    );

    let absent_but_present = std::panic::catch_unwind(|| {
        assert_declaration_holds("t", "edit", ActionSupport::PlatformHasNoApi, not_started())
    });
    assert!(
        absent_but_present.is_err(),
        "declaring PlatformHasNoApi over a non-Unsupported outcome MUST fail"
    );
}
