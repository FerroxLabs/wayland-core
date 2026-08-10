//! P2 — consent is a function of the WHOLE admission shape, and the startup
//! gate cannot be switched off by making its input disappear.
//!
//! Four bypasses of the first cut of this gate, each driven through the
//! production `channel_inbound_host::spawn` over real files on disk.
//!
//! | Leg | Bypass |
//! |---|---|
//! | V-A | acknowledge a narrow open shape, then WIDEN a field the token never named — the admitted set grows from "anyone who can reach G1" to "anyone, anywhere" and nothing refuses |
//! | V-A' | the same machinery must NOT refuse a cosmetic reordering, or an operator drowning in spurious refusals reaches for the nuclear option |
//! | V-B | acknowledge `dm = "open"` on a channel that is switched OFF (and so admits nobody), then flip `enabled = true` — everyone is admitted with no new consent |
//! | V-D | the same token written twice is accepted, so the list is a bag and not the set the docs claim |
//! | V-C | ONE unparseable sibling `.toml` empties the config list, the gate sees zero channels, and an adjacent open channel starts with no refusal and no warning |
//!
//! # Every leg drives the remedy the product itself printed
//!
//! [`advertised_ack`] scrapes the `acknowledge_open_admission = [...]` line out
//! of the refusal and writes exactly that back. Nothing here hard-codes a token
//! spelling, so these legs test the RULE (consent is bound to the whole shape)
//! rather than one encoding of it — and they double as the check that the
//! refusal's own instructions are the ones the gate accepts.
//!
//! `WAYLAND_HOME` is process-global, so every leg runs inside ONE test function
//! rather than several; nextest would otherwise interleave them.

use std::path::{Path, PathBuf};

use wcore_agent::channel_inbound_host::spawn;
use wcore_channels::ChannelManager;
use wcore_config::config::Config;

/// Write `<channels>/<name>.toml`. `enabled` is a separate argument because it
/// is one of the fields under test.
fn write_channel(dir: &Path, name: &str, enabled: bool, inbound: &str) {
    std::fs::create_dir_all(dir).expect("create channels dir");
    std::fs::write(
        dir.join(format!("{name}.toml")),
        format!(
            r#"name = "{name}"
platform = "slack"
enabled = {enabled}

[options]
workspace_name = "p2"
default_channel_id = "D0"
credential_handle_bot_token = "slack.{name}.bot_token"
credential_handle_signing_secret = "slack.{name}.signing_secret"

[inbound]
{inbound}
"#
        ),
    )
    .expect("write channel config");
}

/// Remove every `*.toml` under `dir`, then write one channel. Keeps each leg
/// independent of the last without a fresh profile home per leg.
fn only_channel(dir: &Path, name: &str, enabled: bool, inbound: &str) {
    clear(dir);
    write_channel(dir, name, enabled, inbound);
}

fn clear(dir: &Path) {
    if !dir.exists() {
        return;
    }
    for entry in std::fs::read_dir(dir).expect("read channels dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            std::fs::remove_file(path).expect("clear previous channel config");
        }
    }
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

/// Start over whatever is currently on disk, returning the refusal text.
/// Panics if the start SUCCEEDS — `InboundHost` is not `Debug`, so this cannot
/// be an `expect_err`.
async fn must_refuse(workspace: &str, why: &str) -> String {
    match spawn(manager(), &test_config(), workspace.to_string()).await {
        Ok(_) => panic!("{why}"),
        Err(e) => e.to_string(),
    }
}

/// Start over whatever is currently on disk, and assert the given number of
/// channel policies were loaded. The count is the control that makes a
/// successful start mean "the gate looked at this config and passed it" rather
/// than "the gate saw nothing".
async fn must_start(workspace: &str, expect_policies: usize, why: &str) {
    match spawn(manager(), &test_config(), workspace.to_string()).await {
        Ok(host) => {
            assert_eq!(
                host.policies_loaded, expect_policies,
                "{why}: started, but with {} policies loaded rather than {expect_policies} — a \
                 start over an EMPTY config set proves nothing about the gate",
                host.policies_loaded
            );
            host.shutdown();
        }
        Err(e) => panic!("{why}; got: {e}"),
    }
}

/// The `acknowledge_open_admission = [...]` list the refusal tells the operator
/// to write, returned WITHOUT the brackets so a leg can duplicate or re-emit it.
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

#[tokio::test]
async fn consent_is_bound_to_the_whole_admission_shape_and_the_gate_cannot_be_emptied() {
    let profile = tempfile::tempdir().expect("profile home");
    let channels = PathBuf::from(profile.path()).join("channels");
    let workspace = profile.path().join("work").display().to_string();

    // SAFETY: single-threaded test body; no other thread in this integration
    // test binary reads the environment concurrently. Restored at the end.
    let prev = std::env::var_os("WAYLAND_HOME");
    unsafe {
        std::env::set_var("WAYLAND_HOME", profile.path());
    }

    // ================================================================ V-A
    // Acknowledge a NARROW open shape, then widen a different field.
    //
    // `sender_allowlist = ["*"]` over `group_allowlist = ["G1"]` admits anyone
    // who can reach G1 — bounded by the conversation. Changing
    // `group_allowlist` to `["*"]` grows that to anyone, anywhere. The old cut
    // of the gate tokenised only the fields it considered "open", so this edit
    // changed no token and nothing refused.
    let narrow = |group_allowlist: &str, ack: Option<&str>| {
        let mut body = format!(
            "dm = \"allowlist\"\ndm_allowlist = [\"U-NAMED\"]\ngroup = \"allowlist\"\n\
             group_allowlist = {group_allowlist}\nsender_allowlist = [\"*\"]"
        );
        if let Some(a) = ack {
            body.push_str(&format!("\nacknowledge_open_admission = [{a}]"));
        }
        body
    };

    only_channel(&channels, "widen", true, &narrow("[\"G1\"]", None));
    let msg = must_refuse(
        &workspace,
        "V-A precondition: an unacknowledged open shape must refuse",
    )
    .await;
    let ack_g1 = advertised_ack(&msg);

    only_channel(&channels, "widen", true, &narrow("[\"G1\"]", Some(&ack_g1)));
    must_start(
        &workspace,
        1,
        "V-A precondition: the acknowledgement the refusal printed must let the narrow shape start",
    )
    .await;

    // THE BYPASS. One field widened, same acknowledgement.
    only_channel(&channels, "widen", true, &narrow("[\"*\"]", Some(&ack_g1)));
    let msg = must_refuse(
        &workspace,
        "V-A: widening group_allowlist from [\"G1\"] to [\"*\"] grows the admitted set from \
         \"anyone who can reach G1\" to \"anyone, anywhere\". Consent to the narrow shape must \
         NOT cover it",
    )
    .await;
    assert!(
        msg.contains("group_allowlist"),
        "V-A: the refusal must name the field that changed, not just say the config does not \
         match; got: {msg}"
    );
    assert!(
        msg.contains("G1"),
        "V-A: the refusal must print what was ACKNOWLEDGED, so the operator can see the change \
         rather than an opaque mismatch; got: {msg}"
    );

    // ================================================================ V-A'
    // The other direction: a cosmetic reordering must NOT refuse. A gate that
    // cries wolf on `["G1","G2"]` -> `["G2","G1"]` teaches operators to reach
    // for the nuclear option, which is the failure mode this lane exists to
    // prevent.
    only_channel(&channels, "widen", true, &narrow("[\"G1\", \"G2\"]", None));
    let msg = must_refuse(&workspace, "V-A' precondition: still unacknowledged").await;
    let ack_two = advertised_ack(&msg);

    only_channel(
        &channels,
        "widen",
        true,
        &narrow("[\"G1\", \"G2\"]", Some(&ack_two)),
    );
    must_start(
        &workspace,
        1,
        "V-A' precondition: the two-group shape starts",
    )
    .await;

    only_channel(
        &channels,
        "widen",
        true,
        &narrow("[\"G2\", \"G1\"]", Some(&ack_two)),
    );
    must_start(
        &workspace,
        1,
        "V-A': reordering an allowlist admits exactly the same principals, so it must NOT demand \
         fresh consent",
    )
    .await;

    only_channel(
        &channels,
        "widen",
        true,
        &narrow("[\"G2\", \"G1\", \"G1\"]", Some(&ack_two)),
    );
    must_start(
        &workspace,
        1,
        "V-A': a duplicated entry admits exactly the same principals and must not refuse either",
    )
    .await;

    // ================================================================ V-B
    // Pre-arm through `enabled = false`.
    //
    // A switched-off channel admits NOBODY, so a consent written against it is
    // not contemporaneous with anything. Flipping one word later must be a new
    // decision.
    only_channel(
        &channels,
        "prearm",
        false,
        "dm = \"open\"\ngroup = \"disabled\"",
    );
    let msg = must_refuse(
        &workspace,
        "V-B precondition: a disabled channel's open shape is still checked",
    )
    .await;
    let ack_off = advertised_ack(&msg);

    only_channel(
        &channels,
        "prearm",
        false,
        &format!("dm = \"open\"\ngroup = \"disabled\"\nacknowledge_open_admission = [{ack_off}]"),
    );
    must_start(
        &workspace,
        1,
        "V-B precondition: the acknowledged, switched-off channel starts",
    )
    .await;

    // THE BYPASS. One word, and everyone is admitted.
    only_channel(
        &channels,
        "prearm",
        true,
        &format!("dm = \"open\"\ngroup = \"disabled\"\nacknowledge_open_admission = [{ack_off}]"),
    );
    let msg = must_refuse(
        &workspace,
        "V-B: a consent written while the channel was switched OFF (admitting nobody) must not \
         carry over to the channel being switched ON (admitting everyone)",
    )
    .await;
    assert!(
        msg.contains("enabled"),
        "V-B: the refusal must name `enabled` as what changed; got: {msg}"
    );

    // And it is a refusal, not a wedge: acknowledging the shape as it now
    // stands lets the operator through.
    let ack_on = advertised_ack(&msg);
    assert_ne!(
        ack_on, ack_off,
        "V-B: the two shapes admit different sets of principals, so they must not share a token"
    );
    only_channel(
        &channels,
        "prearm",
        true,
        &format!("dm = \"open\"\ngroup = \"disabled\"\nacknowledge_open_admission = [{ack_on}]"),
    );
    must_start(
        &workspace,
        1,
        "V-B: re-acknowledging the live shape must let it start",
    )
    .await;

    // CONTROL. `enabled` is only part of a consent when there is something to
    // consent to. A bounded channel must not acquire a consent requirement just
    // by being switched on and off.
    only_channel(
        &channels,
        "prearm",
        false,
        "dm = \"allowlist\"\ndm_allowlist = [\"U-NAMED\"]\ngroup = \"disabled\"",
    );
    must_start(&workspace, 1, "control: a bounded, disabled channel starts").await;
    only_channel(
        &channels,
        "prearm",
        true,
        "dm = \"allowlist\"\ndm_allowlist = [\"U-NAMED\"]\ngroup = \"disabled\"",
    );
    must_start(
        &workspace,
        1,
        "control: switching a BOUNDED channel on must not demand a consent it never needed",
    )
    .await;

    // ================================================================ V-D
    // The same token twice. The docs say the list MATCHES the open set; a bag
    // that happens to contain the right elements is not a set.
    only_channel(
        &channels,
        "dupes",
        true,
        "dm = \"open\"\ngroup = \"disabled\"",
    );
    let msg = must_refuse(
        &workspace,
        "V-D precondition: unacknowledged open channel refuses",
    )
    .await;
    let ack_one = advertised_ack(&msg);

    only_channel(
        &channels,
        "dupes",
        true,
        &format!(
            "dm = \"open\"\ngroup = \"disabled\"\nacknowledge_open_admission = [{ack_one}, {ack_one}]"
        ),
    );
    let msg = must_refuse(
        &workspace,
        "V-D: a duplicated acknowledgement is not the exact match the design requires, and \
         accepting it makes the list a bag rather than a set",
    )
    .await;
    assert!(
        msg.contains("acknowledge_open_admission"),
        "V-D: the refusal must name the key at fault; got: {msg}"
    );

    // ================================================================ V-C
    // One unparseable sibling turns the whole gate off.
    //
    // The admission consequence is fail-CLOSED (an empty registry denies
    // everyone), so this is not an admits-everyone hole. It is worse in a
    // different way: a security gate that any stray file in the directory can
    // silently satisfy is not a gate, and the same typo silently converts a
    // working gateway into universal denial at the next restart.
    let bounded = "dm = \"allowlist\"\ndm_allowlist = [\"U-NAMED\"]\ngroup = \"disabled\"";

    // CONTROL 1 — the bounded channel alone starts, and is actually loaded.
    only_channel(&channels, "neighbour", true, bounded);
    must_start(
        &workspace,
        1,
        "V-C control: the bounded channel alone starts",
    )
    .await;

    // CONTROL 2 — the open channel alone refuses. This is what the junk file
    // must not be able to switch off.
    only_channel(
        &channels,
        "wideopen",
        true,
        "dm = \"open\"\ngroup = \"disabled\"",
    );
    must_refuse(
        &workspace,
        "V-C control: an open channel ALONE must refuse, or the leg below is vacuous",
    )
    .await;

    // THE BYPASS. Same open channel, one junk sibling.
    std::fs::write(channels.join("junk.toml"), "this is not = valid toml [[[\n")
        .expect("write unparseable sibling");
    let msg = must_refuse(
        &workspace,
        "V-C: an unparseable sibling emptied the config list, so the gate saw zero channels and \
         the adjacent open channel started unrefused",
    )
    .await;
    assert!(
        msg.contains("junk.toml"),
        "V-C: the operator must be told WHICH file cannot be parsed; got: {msg}"
    );

    // And the same loudness for a directory with nothing open at all: a config
    // that cannot be parsed is an error the operator sees, never an empty list.
    only_channel(&channels, "neighbour", true, bounded);
    std::fs::write(channels.join("junk.toml"), "this is not = valid toml [[[\n")
        .expect("write unparseable sibling");
    let msg = must_refuse(
        &workspace,
        "V-C: one typo must not silently convert a working gateway into universal denial at the \
         next restart",
    )
    .await;
    assert!(
        msg.contains("junk.toml"),
        "V-C: and that failure must name the file too; got: {msg}"
    );

    // CONTROL 3 — remove the junk and the same directory starts again. Proves
    // the two legs above are about the unparseable file and not about the
    // profile home having become unreadable.
    std::fs::remove_file(channels.join("junk.toml")).expect("remove the junk sibling");
    must_start(
        &workspace,
        1,
        "V-C control: withdrawing the unparseable file must let the gateway start again",
    )
    .await;

    clear(&channels);
    unsafe {
        match prev {
            Some(v) => std::env::set_var("WAYLAND_HOME", v),
            None => std::env::remove_var("WAYLAND_HOME"),
        }
    }
}
