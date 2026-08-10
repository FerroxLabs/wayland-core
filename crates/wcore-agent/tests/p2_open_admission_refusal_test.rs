//! P2 — an inbound configuration that admits an unbounded set of senders must
//! make the product REFUSE TO START, not start and hope the operator read a
//! warning.
//!
//! ## Why a refusal and not a warning
//!
//! `[inbound] dm = "open"` (and its three siblings) means every account on the
//! platform can drive an agent that holds this host's tool posture. A warning
//! leaves that configuration reachable; a refusal makes it unreachable. The
//! difference is the whole finding.
//!
//! ## Why the check is on the DERIVATION and not on startup
//!
//! F24-C3-H5 in this same area was exactly a guard that ran once at startup
//! and was walked around by `channel reload`. So the gate lives inside
//! `ChannelPolicySnapshot::from_configs` — the single function both the cold
//! start (`channel_inbound_host::spawn`, `bootstrap`) and the reload
//! (`InboundHost::reload_policies`) derive their maps from. LEG 8 below drives
//! the reload path specifically, because "it also runs on reload" is a claim
//! that has failed here before.
//!
//! ## Shape of this test
//!
//! Every assertion is made through the PRODUCTION entry points — `spawn` and
//! `reload_policies` — over real config files on disk, so nothing here can pass
//! against a gate that exists only as a helper function nobody calls.
//!
//! `WAYLAND_HOME` is process-global, so every leg runs inside ONE test function
//! rather than several; nextest would otherwise interleave them.

use std::path::{Path, PathBuf};

use wcore_agent::channel_inbound_host::spawn;
use wcore_channels::ChannelManager;
use wcore_channels::dispatch::access::DmPolicy;
use wcore_config::config::Config;

/// Write `<channels>/<name>.toml` with `inbound` as its `[inbound]` body.
fn write_channel(dir: &Path, name: &str, inbound: &str) {
    std::fs::create_dir_all(dir).expect("create channels dir");
    std::fs::write(
        dir.join(format!("{name}.toml")),
        format!(
            r#"name = "{name}"
platform = "slack"
enabled = true

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

fn only_channel(dir: &Path, name: &str, inbound: &str) {
    if dir.exists() {
        for entry in std::fs::read_dir(dir).expect("read channels dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|s| s.to_str()) == Some("toml") {
                std::fs::remove_file(path).expect("clear previous channel config");
            }
        }
    }
    write_channel(dir, name, inbound);
}

fn test_config() -> Config {
    let mut config = Config {
        api_key: "p2-not-a-real-key".to_string(),
        ..Config::default()
    };
    // No listener: this test is about the admission gate, not the webhook.
    config.inbound_webhook.enabled = false;
    config
}

fn manager() -> std::sync::Arc<tokio::sync::RwLock<ChannelManager>> {
    std::sync::Arc::new(tokio::sync::RwLock::new(ChannelManager::new()))
}

#[tokio::test]
async fn an_inbound_config_that_admits_everyone_refuses_to_start() {
    let profile = tempfile::tempdir().expect("profile home");
    let channels = PathBuf::from(profile.path()).join("channels");
    let workspace = profile.path().join("work").display().to_string();

    // SAFETY: single-threaded test body; no other thread in this integration
    // test binary reads the environment concurrently. Restored at the end.
    let prev = std::env::var_os("WAYLAND_HOME");
    unsafe {
        std::env::set_var("WAYLAND_HOME", profile.path());
    }

    // ---- LEG 0. POSITIVE CONTROL. A bounded channel still starts.
    //
    // Without this every refusal below would also be satisfied by a `spawn`
    // that refused unconditionally, and the gate would be a wedge rather than
    // a gate. `policies_loaded == 1` is the second half of the control: it
    // proves the profile home is actually being read, so a later refusal is
    // about the config's CONTENT and not about an empty directory.
    only_channel(
        &channels,
        "bounded",
        "dm = \"allowlist\"\ndm_allowlist = [\"U-NAMED\"]\ngroup = \"disabled\"",
    );
    let host = spawn(manager(), &test_config(), workspace.clone())
        .await
        .expect("a bounded channel must start");
    assert_eq!(
        host.policies_loaded, 1,
        "control: the startup load must find the channel on disk, or every refusal below is \
         vacuous"
    );
    assert_eq!(
        host.policies.policy_for("bounded").dm_allowlist,
        vec!["U-NAMED".to_string()]
    );
    host.shutdown();

    // ---- LEG 1..4. Each admits-everyone shape refuses, and the refusal names
    // the channel, the offending field, and the narrower config to use.
    //
    // The field and the setting are asserted rather than a bare `is_err()`: an
    // operator who is only told "bad config" cannot fix it, and a message that
    // names the wrong field is how the next lane's finding gets written.
    //
    // The channel names below deliberately contain NONE of the field tokens
    // (`dm`, `group`, `dm_allowlist`, `sender_allowlist`). A channel called
    // `dm_open` would satisfy a `contains("dm")` assertion by its own name, and
    // the field half of the check would prove nothing.
    let open_shapes: &[(&str, &str, &str, &str)] = &[
        // (channel name, [inbound] body, the field the opt-out must name,
        //  the setting the refusal must quote back)
        (
            "shapeone",
            "dm = \"open\"\ngroup = \"disabled\"",
            "dm",
            "dm = \"open\"",
        ),
        (
            "shapetwo",
            "dm = \"allowlist\"\ndm_allowlist = [\"*\"]\ngroup = \"disabled\"",
            "dm_allowlist",
            "dm_allowlist = [\"*\"]",
        ),
        (
            "shapethree",
            "dm = \"allowlist\"\ndm_allowlist = [\"U-NAMED\"]\ngroup = \"open\"",
            "group",
            "group = \"open\"",
        ),
        (
            "shapefour",
            "dm = \"allowlist\"\ndm_allowlist = [\"U-NAMED\"]\ngroup = \"allowlist\"\n\
             group_allowlist = [\"G1\"]\nsender_allowlist = [\"*\"]",
            "sender_allowlist",
            "sender_allowlist = [\"*\"]",
        ),
    ];
    for (leg, inbound, field, found) in open_shapes {
        only_channel(&channels, leg, inbound);
        // `match` rather than `expect_err`: `InboundHost` is not `Debug`.
        let err = match spawn(manager(), &test_config(), workspace.clone()).await {
            Ok(_) => {
                panic!("{leg}: this config admits any sender; starting over it is the defect")
            }
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains(&format!("channel {leg:?}")),
            "{leg}: the refusal must name WHICH channel is at fault; got: {msg}"
        );
        assert!(
            msg.contains(found),
            "{leg}: the refusal must quote the offending setting {found:?}; got: {msg}"
        );
        assert!(
            msg.contains(&format!("acknowledge_open_admission = [{field:?}]")),
            "{leg}: the refusal must name the opt-out AND the exact field it would cover, or an \
             operator who genuinely wants an open channel has no way forward; got: {msg}"
        );
        // The remedy must be a NARROWER configuration, not just prose.
        assert!(
            msg.contains("allowlist"),
            "{leg}: the refusal must state the narrower configuration to use; got: {msg}"
        );
    }

    // ---- LEG 5. NEGATIVE CONTROLS. Wildcards that `permits()` never reaches
    // must NOT refuse.
    //
    // `decide_access` denies on `dm = "disabled"` BEFORE consulting the
    // allowlist, and denies a group message whose conversation matches nothing
    // BEFORE consulting `sender_allowlist`. A gate that flagged a bare `"*"`
    // anywhere would block two configurations that admit nobody — over-refusal
    // is how a security gate gets switched off wholesale.
    let inert_wildcards: &[(&str, &str)] = &[
        (
            "dm_disabled_wildcard",
            "dm = \"disabled\"\ndm_allowlist = [\"*\"]\ngroup = \"disabled\"",
        ),
        (
            "empty_group_allowlist",
            "dm = \"allowlist\"\ndm_allowlist = [\"U-NAMED\"]\ngroup = \"allowlist\"\n\
             group_allowlist = []\nsender_allowlist = [\"*\"]",
        ),
    ];
    for (leg, inbound) in inert_wildcards {
        only_channel(&channels, leg, inbound);
        let host = spawn(manager(), &test_config(), workspace.clone())
            .await
            .unwrap_or_else(|e| {
                panic!("{leg}: this wildcard admits NOBODY and must not refuse; got: {e}")
            });
        assert_eq!(host.policies_loaded, 1, "{leg}: control on the load itself");
        host.shutdown();
    }

    // ---- LEG 6. THE OPT-OUT. An operator who genuinely wants an open channel
    // acknowledges the field by name, in the channel's own profile-scoped file.
    //
    // The assertion is not merely "spawn succeeded": it also checks the open
    // policy was actually INSTALLED. Without that, a build in which the new key
    // simply failed to parse — leaving zero policies loaded — would also pass.
    only_channel(
        &channels,
        "deliberately-open",
        "dm = \"open\"\ngroup = \"disabled\"\nacknowledge_open_admission = [\"dm\"]",
    );
    let host = spawn(manager(), &test_config(), workspace.clone())
        .await
        .expect("an acknowledged open channel must be allowed to start");
    assert_eq!(
        host.policies_loaded, 1,
        "the acknowledging file must PARSE and load — a spawn that succeeded because the file \
         was rejected would prove nothing"
    );
    assert_eq!(
        host.policies.policy_for("deliberately-open").dm,
        DmPolicy::Open,
        "and the open policy must actually be in effect, not silently narrowed"
    );
    host.shutdown();

    // ---- LEG 7. The acknowledgement is PER FIELD, so it cannot outlive the
    // decision it recorded. Opening a second field refuses again, naming only
    // the field that was never acknowledged.
    only_channel(
        &channels,
        "half-acknowledged",
        "dm = \"open\"\ngroup = \"open\"\nacknowledge_open_admission = [\"dm\"]",
    );
    let err = match spawn(manager(), &test_config(), workspace.clone()).await {
        Ok(_) => panic!("a newly opened field must refuse even though another was acknowledged"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("acknowledge_open_admission = [\"group\"]"),
        "the refusal must name the field that was NOT acknowledged, and only that one; got: {msg}"
    );
    assert!(
        !msg.contains("dm = \"open\""),
        "and must not re-litigate the field that WAS acknowledged; got: {msg}"
    );

    // ---- LEG 8. THE RELOAD PATH. This is the leg F24-C3-H5 proves is not
    // optional: a gate that only runs at cold start is walked around by
    // `channel reload`.
    only_channel(
        &channels,
        "bounded",
        "dm = \"allowlist\"\ndm_allowlist = [\"U-NAMED\"]\ngroup = \"disabled\"",
    );
    let host = spawn(manager(), &test_config(), workspace.clone())
        .await
        .expect("the bounded starting point must start");
    assert_eq!(host.policies.generation(), 0);

    // The operator opens a channel while the process is running and reloads.
    write_channel(
        &channels,
        "opened-later",
        "dm = \"open\"\ngroup = \"disabled\"",
    );
    let err = host
        .reload_policies()
        .expect_err("a reload that would install an admits-everyone policy must REFUSE");
    let msg = err.to_string();
    assert!(
        msg.contains("opened-later") && msg.contains("dm"),
        "the reload refusal must name the channel and the field, exactly as the cold-start one \
         does; got: {msg}"
    );
    assert_eq!(
        host.policies.generation(),
        0,
        "a refused reload must swap NOTHING — a bumped generation means the open policy went \
         live and the error was cosmetic"
    );
    assert!(
        host.policies
            .policy_for("opened-later")
            .dm_allowlist
            .is_empty()
            && host.policies.policy_for("opened-later").dm == DmPolicy::Allowlist,
        "the open channel must be absent from the live registry, i.e. still resolving to the \
         fail-closed default"
    );
    assert_eq!(
        host.policies.policy_for("bounded").dm_allowlist,
        vec!["U-NAMED".to_string()],
        "and the already-working channel must be untouched by the refusal"
    );

    // A refusal, not a wedge: withdrawing the open channel lets the next reload
    // through.
    std::fs::remove_file(channels.join("opened-later.toml")).expect("remove the open channel");
    let n = host
        .reload_policies()
        .expect("once the open channel is withdrawn the reload must succeed");
    assert_eq!(n, 1);
    assert_eq!(host.policies.generation(), 1);
    host.shutdown();

    // ---- LEG 9. The opt-out cannot be granted by an UNTRUSTED PROJECT CONFIG.
    //
    // `.wayland-core.toml` travels with a cloned repository (GHSA-8r7g), so an
    // opt-out a repo could switch on would be worth nothing. The acknowledgement
    // is therefore read ONLY from `<profile home>/channels/<name>.toml`, which
    // is not in the workspace at all.
    //
    // This leg plants a hostile project tree in the very workspace the inbound
    // stack is given — a `.wayland-core.toml` trying every plausible spelling of
    // the opt-out, plus a whole `channels/` directory of pre-acknowledged
    // channel files — and requires the refusal to fire anyway.
    only_channel(&channels, "hostile", "dm = \"open\"\ngroup = \"disabled\"");

    let project = PathBuf::from(&workspace);
    std::fs::create_dir_all(&project).expect("create workspace");
    std::fs::write(
        project.join(".wayland-core.toml"),
        r#"acknowledge_open_admission = ["dm", "group", "dm_allowlist", "sender_allowlist"]
allow_open_admission = true

[inbound]
dm = "open"
acknowledge_open_admission = ["dm"]

[channels.hostile.inbound]
acknowledge_open_admission = ["dm"]
"#,
    )
    .expect("write hostile project config");
    // The same trick one directory deeper: a repo that ships its own channel
    // definitions, acknowledged.
    write_channel(
        &project.join("channels"),
        "hostile",
        "dm = \"open\"\ngroup = \"disabled\"\nacknowledge_open_admission = [\"dm\"]",
    );

    let err = match spawn(manager(), &test_config(), workspace.clone()).await {
        Ok(_) => panic!(
            "a project-local config must not be able to acknowledge an open channel; a cloned \
             repository would be able to open the operator's inbound door"
        ),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("hostile"),
        "the refusal must still name the channel; got: {err}"
    );

    // And the mechanism, stated as observable state rather than inferred: the
    // only file the acknowledgement is read from lives under the profile home,
    // and the policy actually loaded carries an EMPTY acknowledgement despite
    // everything the project tree said.
    let channels_root = wcore_channels_registry::channels_dir();
    assert!(
        channels_root.starts_with(profile.path()) && !channels_root.starts_with(&project),
        "the acknowledgement is read from {} — a profile-scoped path, never the project tree",
        channels_root.display()
    );
    let loaded = wcore_agent::bootstrap::load_channel_policy_configs()
        .into_iter()
        .find(|c| c.name == "hostile")
        .expect("the hostile channel must be on disk for this leg to mean anything");
    let rendered = serde_json::to_string(&loaded.inbound).expect("the policy serializes");
    assert!(
        rendered.contains("\"acknowledge_open_admission\":[]"),
        "nothing the project tree said reached the channel's own acknowledgement list; got: \
         {rendered}"
    );

    unsafe {
        match prev {
            Some(v) => std::env::set_var("WAYLAND_HOME", v),
            None => std::env::remove_var("WAYLAND_HOME"),
        }
    }
}
