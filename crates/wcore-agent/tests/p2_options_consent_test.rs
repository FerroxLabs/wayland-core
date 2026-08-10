//! P2/N1 — the consent token covers the ADAPTER's own admission filter, not
//! only `[inbound]`.
//!
//! # The defect
//!
//! `[inbound]` is not the only place a channel decides who is admitted. Four of
//! the ten adapters carry a SECOND admission filter in their own `[options]`
//! table, and in every one of them an ABSENT or EMPTY list is the MOST
//! permissive state:
//!
//! | Adapter | Key | Enforcement | Empty means |
//! |---|---|---|---|
//! | email | `options.imap.allowed_senders` | `imap.rs` `is_sender_allowed` | every `From:` admitted |
//! | discord | `options.allowed_channel_ids` | `gateway.rs` `map_message_create` | every channel admitted |
//! | telegram | `options.allowed_chat_ids` | long-poll layer | every chat admitted |
//! | imessage | `options.allowed_handles` | poll layer | every handle admitted |
//!
//! The whole-shape token rendered `[inbound]` plus `enabled` and nothing else,
//! so `[options.imap] allowed_senders = ["boss@acme.test"]` and the same file
//! with that ONE line deleted produced a BYTE-IDENTICAL token — and the widened
//! config started on the narrow config's consent. That is the V-A defect again,
//! one config section over.
//!
//! # What these legs drive
//!
//! The production `channel_inbound_host::spawn`, over real files on disk, for
//! all four adapters. Nothing here hard-codes a token spelling: every leg
//! scrapes the `acknowledge_open_admission = [...]` line out of the product's
//! own refusal and writes exactly that back, so the legs test the RULE rather
//! than one encoding of it, and double as the check that the refusal's
//! instructions are the ones the gate accepts.
//!
//! `WAYLAND_HOME` is process-global, so every leg runs inside ONE test function
//! rather than several; nextest would otherwise interleave them.

use std::path::{Path, PathBuf};

use wcore_agent::channel_inbound_host::spawn;
use wcore_channels::ChannelManager;
use wcore_config::config::Config;

/// Write `<channels>/<name>.toml` with an explicit `[options]` body, after
/// clearing every sibling — so each leg is independent of the last.
fn only_channel(dir: &Path, name: &str, platform: &str, options: &str, inbound: &str) {
    if dir.exists() {
        for entry in std::fs::read_dir(dir).expect("read channels dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                std::fs::remove_file(path).expect("clear previous channel config");
            }
        }
    }
    std::fs::create_dir_all(dir).expect("create channels dir");
    // `[options]` goes LAST: it is the section whose sub-tables
    // (`[options.imap]`, …) a leg varies, and a trailing sub-table must not
    // swallow the keys of whatever followed it.
    std::fs::write(
        dir.join(format!("{name}.toml")),
        format!(
            "name = \"{name}\"\nplatform = \"{platform}\"\nenabled = true\n\n[inbound]\n\
             {inbound}\n\n[options]\n{options}"
        ),
    )
    .expect("write channel config");
}

fn test_config() -> Config {
    let mut config = Config {
        api_key: "p2-not-a-real-key".to_string(),
        ..Config::default()
    };
    config.inbound_webhook.enabled = false;
    config
}

fn manager() -> std::sync::Arc<tokio::sync::RwLock<ChannelManager>> {
    std::sync::Arc::new(tokio::sync::RwLock::new(ChannelManager::new()))
}

/// Start over whatever is on disk; panic if it SUCCEEDS. Returns the refusal.
/// (`InboundHost` is not `Debug`, so this cannot be an `expect_err`.)
async fn must_refuse(workspace: &str, why: &str) -> String {
    match spawn(manager(), &test_config(), workspace.to_string()).await {
        Ok(_) => panic!("{why}"),
        Err(e) => e.to_string(),
    }
}

/// Start over whatever is on disk, asserting ONE policy was loaded. The count
/// is the control that makes a successful start mean "the gate looked at this
/// config and passed it" rather than "the gate saw nothing".
async fn must_start(workspace: &str, why: &str) {
    match spawn(manager(), &test_config(), workspace.to_string()).await {
        Ok(host) => {
            assert_eq!(
                host.policies_loaded, 1,
                "{why}: started, but with {} policies loaded rather than 1 — a start over an \
                 EMPTY config set proves nothing about the gate",
                host.policies_loaded
            );
            host.shutdown();
        }
        Err(e) => panic!("{why}; got: {e}"),
    }
}

/// The `acknowledge_open_admission = [...]` list the refusal tells the operator
/// to write, without the brackets.
fn advertised_ack(msg: &str) -> String {
    const KEY: &str = "acknowledge_open_admission = [";
    let at = msg.find(KEY).unwrap_or_else(|| {
        panic!(
            "the refusal must print the acknowledgement that would cover the configuration as it \
             stands, or an operator who genuinely wants an open channel has no way forward; got: \
             {msg}"
        )
    });
    let rest = &msg[at + KEY.len()..];
    let end = rest
        .find(']')
        .unwrap_or_else(|| panic!("unterminated acknowledgement list in refusal: {msg}"));
    let list = rest[..end].trim().to_string();
    assert!(
        !list.is_empty(),
        "the refusal advertised an EMPTY acknowledgement list for a channel it refused as open: \
         {msg}"
    );
    list
}

/// The `[inbound]` half is OPEN in every leg, because that is what makes a
/// token required at all. The variable under test is the `[options]` half.
fn inbound(ack: Option<&str>) -> String {
    let mut body = "dm = \"open\"".to_string();
    if let Some(a) = ack {
        body.push_str(&format!("\nacknowledge_open_admission = [{a}]"));
    }
    body
}

/// The email `[options]` body. `imap_allowlist` is spliced in verbatim so a leg
/// can OMIT the line entirely rather than merely emptying it — deletion is the
/// widening this defect is about.
fn email_options(imap_allowlist: &str) -> String {
    format!(
        "from_address = \"bot@acme.test\"\n\n[options.smtp]\nhost = \"smtp.acme.test\"\n\
         user_credential_handle = \"email.u\"\npassword_credential_handle = \"email.p\"\n\n\
         [options.imap]\nhost = \"imap.acme.test\"\nuser_credential_handle = \"email.u\"\n\
         password_credential_handle = \"email.p\"\n{imap_allowlist}"
    )
}

/// One adapter-level admission filter: the narrow `[options]`, the same table
/// with the filter DELETED, and the token path the refusal must name.
struct Filter {
    platform: &'static str,
    narrow: String,
    widened: String,
    path: &'static str,
}

fn filters() -> Vec<Filter> {
    vec![
        Filter {
            platform: "email",
            narrow: email_options("allowed_senders = [\"boss@acme.test\"]\n"),
            widened: email_options(""),
            path: "options.imap.allowed_senders",
        },
        Filter {
            platform: "discord",
            narrow: "credential_handle = \"discord.tok\"\nallowed_channel_ids = [\"111\"]\n"
                .to_string(),
            widened: "credential_handle = \"discord.tok\"\n".to_string(),
            path: "options.allowed_channel_ids",
        },
        Filter {
            platform: "telegram",
            narrow: "credential_handle = \"tg.tok\"\nallowed_chat_ids = [\"-1001\"]\n".to_string(),
            widened: "credential_handle = \"tg.tok\"\n".to_string(),
            path: "options.allowed_chat_ids",
        },
        Filter {
            platform: "imessage",
            narrow: "allowed_handles = [\"+15550000000\"]\n".to_string(),
            widened: String::new(),
            path: "options.allowed_handles",
        },
    ]
}

#[tokio::test]
async fn consent_covers_each_adapters_own_admission_filter() {
    let profile = tempfile::tempdir().expect("profile home");
    let channels = PathBuf::from(profile.path()).join("channels");
    let workspace = profile.path().join("work").display().to_string();

    // SAFETY: single-threaded test body; no other thread in this integration
    // test binary reads the environment concurrently. Restored at the end.
    let prev = std::env::var_os("WAYLAND_HOME");
    unsafe {
        std::env::set_var("WAYLAND_HOME", profile.path());
    }

    for Filter {
        platform,
        narrow,
        widened,
        path,
    } in filters()
    {
        // ---- PRECONDITION: the narrow config, unacknowledged, refuses, and
        // ---- the token the refusal advertises lets it start.
        only_channel(&channels, "c", platform, &narrow, &inbound(None));
        let msg = must_refuse(
            &workspace,
            &format!("{platform}: an unacknowledged open shape must refuse"),
        )
        .await;
        let ack_narrow = advertised_ack(&msg);

        only_channel(
            &channels,
            "c",
            platform,
            &narrow,
            &inbound(Some(&ack_narrow)),
        );
        must_start(
            &workspace,
            &format!(
                "{platform}: the acknowledgement the refusal printed must let the narrow \
                      config start"
            ),
        )
        .await;

        // ---- THE BYPASS. Delete the adapter's admission filter. Its list is
        // ---- now absent, which for all four adapters means EVERY sender is
        // ---- admitted, and nothing in `[inbound]` moved.
        only_channel(
            &channels,
            "c",
            platform,
            &widened,
            &inbound(Some(&ack_narrow)),
        );
        let msg = must_refuse(
            &workspace,
            &format!(
                "{platform}: deleting {path} widens the adapter's admitted set from a named list \
                 to EVERYONE. Consent to the narrow config must NOT cover it"
            ),
        )
        .await;
        assert!(
            msg.contains(path),
            "{platform}: the refusal must NAME the key that changed, not just say the config does \
             not match; got: {msg}"
        );
        assert!(
            msg.contains("(absent)"),
            "{platform}: the refusal must say the key is now ABSENT — for these adapters that is \
             the permissive state, and an empty rendering would read like a narrow value; got: \
             {msg}"
        );

        // ---- And the operator has a way forward: the token the refusal
        // ---- printed for the WIDE config is the one the gate accepts.
        let ack_wide = advertised_ack(&msg);
        assert_ne!(
            ack_wide, ack_narrow,
            "{platform}: the two configs admit different sets, so they must not share a token — \
             an identical token here IS the defect"
        );
        only_channel(
            &channels,
            "c",
            platform,
            &widened,
            &inbound(Some(&ack_wide)),
        );
        must_start(
            &workspace,
            &format!("{platform}: the token the refusal advertised must be the one it accepts"),
        )
        .await;

        // ---- THE OTHER DIRECTION. Restoring the filter is also a change to
        // ---- the shape the consent names, so the wide consent must not
        // ---- silently cover the narrow config either.
        only_channel(&channels, "c", platform, &narrow, &inbound(Some(&ack_wide)));
        must_refuse(
            &workspace,
            &format!(
                "{platform}: a consent names ONE configuration; re-adding {path} changes it and \
                 must refuse rather than being covered"
            ),
        )
        .await;
    }

    // ================================================================
    // NO OVER-REFUSAL. A cosmetic reorder or a repeated entry inside an
    // `[options]` list admits exactly the same principals. A gate that cries
    // wolf here teaches operators to stop reading refusals, and an operator who
    // stops reading refusals reaches for the nuclear option.
    let two = "credential_handle = \"discord.tok\"\nallowed_channel_ids = [\"111\", \"222\"]\n";
    only_channel(&channels, "c", "discord", two, &inbound(None));
    let ack_two =
        advertised_ack(&must_refuse(&workspace, "cosmetic precondition: unacknowledged").await);
    only_channel(&channels, "c", "discord", two, &inbound(Some(&ack_two)));
    must_start(
        &workspace,
        "cosmetic precondition: the two-channel config starts",
    )
    .await;

    for cosmetic in [
        "credential_handle = \"discord.tok\"\nallowed_channel_ids = [\"222\", \"111\"]\n",
        "credential_handle = \"discord.tok\"\nallowed_channel_ids = [\"222\", \"111\", \"111\"]\n",
    ] {
        only_channel(
            &channels,
            "c",
            "discord",
            cosmetic,
            &inbound(Some(&ack_two)),
        );
        must_start(
            &workspace,
            "a reordered or repeated [options] list entry admits exactly the same principals and \
             must NOT refuse",
        )
        .await;
    }

    // ================================================================
    // AN EMPTY LIST IS NOT AN ABSENT ONE. Both admit everyone here, but they
    // are different states of the file, and a token that collapsed them would
    // let one be edited into the other under a live consent.
    let empty = "credential_handle = \"discord.tok\"\nallowed_channel_ids = []\n";
    only_channel(&channels, "c", "discord", empty, &inbound(None));
    let ack_empty = advertised_ack(&must_refuse(&workspace, "empty-list precondition").await);
    let absent = "credential_handle = \"discord.tok\"\n";
    only_channel(&channels, "c", "discord", absent, &inbound(None));
    let ack_absent = advertised_ack(&must_refuse(&workspace, "absent-key precondition").await);
    assert_ne!(
        ack_empty, ack_absent,
        "an EMPTY allowed_channel_ids and an ABSENT one are different states of the file and must \
         not share a consent token"
    );
    only_channel(
        &channels,
        "c",
        "discord",
        empty,
        &inbound(Some(&ack_absent)),
    );
    must_refuse(
        &workspace,
        "the consent written for an ABSENT key must not cover a PRESENT-but-empty one",
    )
    .await;

    // SAFETY: same single-threaded body; restore what we found.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WAYLAND_HOME", v),
            None => std::env::remove_var("WAYLAND_HOME"),
        }
    }
}
