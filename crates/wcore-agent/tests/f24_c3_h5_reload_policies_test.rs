//! F24-C3-H5 regression: a channel added while the gateway is running must be
//! able to RECEIVE after `channel reload`, with the tool posture its config
//! asked for.
//!
//! ## What broke
//!
//! `channel reload` rebuilt the adapter set and nothing else. The inbound
//! access policy and the per-channel tool posture were both read once, in
//! `channel_inbound_host::spawn`, and then moved into the spawned subscriber
//! task and into `ChannelTurnDispatcher` — owned `HashMap`s behind no shared
//! handle, unreachable for the life of the process.
//!
//! So a channel introduced by a reload was absent from a map captured before it
//! existed, fell through to `InboundPolicy::default()` — `dm = allowlist` over
//! an EMPTY allowlist, which permits nothing — and had every message silently
//! denied. Meanwhile `channel health` reported it `healthy`, the registration
//! count included it, and its webhook endpoint answered `HTTP 200`. Three
//! surfaces, all wrong, no error and no log an operator would look at.
//!
//! Measured on Linux with a one-variable control: the identical config from the
//! identical generator was ADMITTED when present at gateway start and DENIED
//! when introduced by reload.
//!
//! ## The trap this test exists to close
//!
//! The repair has TWO facets — the policy map and the tool postures — and
//! **fixing only the first passes a naive re-run**: messages start arriving, so
//! an arrivals-only assertion goes green, while the channel runs under the
//! dispatcher's fallback posture instead of its configured one. That outcome is
//! worse than the original bug, because it is not fail-closed.
//!
//! This test therefore asserts the POSTURE, not just the policy, and asserts
//! that a reloaded channel's resolved configuration EQUALS what the same config
//! produces when it is present at startup.
//!
//! `WAYLAND_HOME` is process-global, so every leg runs inside ONE test function
//! rather than several — nextest would otherwise interleave them.

use std::path::{Path, PathBuf};

use wcore_agent::channel_inbound_host::spawn;
use wcore_agent::channel_policy::ChannelPolicyRegistry;
use wcore_channels::ChannelManager;
use wcore_channels::dispatch::access::{ChannelToolPosture, DmPolicy};
use wcore_config::config::Config;

fn write_channel(dir: &Path, name: &str, allow: &str, tools: &str, workspace_root: Option<&str>) {
    std::fs::create_dir_all(dir).expect("create channels dir");
    let root_line = match workspace_root {
        Some(r) => format!("tool_workspace_root = \"{r}\"\n"),
        None => String::new(),
    };
    std::fs::write(
        dir.join(format!("{name}.toml")),
        format!(
            r#"name = "{name}"
platform = "slack"
enabled = true

[options]
workspace_name = "f24c3h5"
default_channel_id = "D0"
credential_handle_bot_token = "slack.{name}.bot_token"
credential_handle_signing_secret = "slack.{name}.signing_secret"

[inbound]
dm = "allowlist"
dm_allowlist = ["{allow}"]
group = "disabled"
require_mention = true
tools = "{tools}"
{root_line}"#
        ),
    )
    .expect("write channel config");
}

#[tokio::test]
async fn a_reloaded_channel_carries_its_policy_and_its_posture() {
    let profile = tempfile::tempdir().expect("profile home");
    let channels = PathBuf::from(profile.path()).join("channels");

    // Present at startup.
    write_channel(&channels, "atstart", "U-AT-START", "conversational", None);

    // SAFETY: single-threaded test body; no other thread in this integration
    // test binary reads the environment concurrently. Restored at the end.
    let prev = std::env::var_os("WAYLAND_HOME");
    unsafe {
        std::env::set_var("WAYLAND_HOME", profile.path());
    }

    let mut config = Config {
        api_key: "f24c3h5-not-a-real-key".to_string(),
        ..Config::default()
    };
    // No listener: this test is about the policy registry, not the webhook.
    config.inbound_webhook.enabled = false;

    let manager = std::sync::Arc::new(tokio::sync::RwLock::new(ChannelManager::new()));
    let workspace = profile.path().join("work").display().to_string();
    let host = spawn(manager, &config, workspace.clone())
        .await
        .expect("the inbound host must build");

    // ---- LEG 1. Startup state, and the known-negative the rest depends on.
    assert_eq!(
        host.policies_loaded, 1,
        "positive control: the startup load must find the one channel on disk. If this is 0 \
         the profile home is not being read and every assertion below is vacuous"
    );
    assert_eq!(host.policies.names(), vec!["atstart".to_string()]);
    assert!(
        host.policies.scope_for("addedlater").is_none(),
        "known-negative: the channel added below must genuinely be absent now, or the reload \
         assertions prove nothing"
    );
    let default_policy = wcore_channels::InboundPolicy::default();
    assert!(
        default_policy.dm_allowlist.is_empty() && default_policy.dm == DmPolicy::Allowlist,
        "control on the fallback itself: an absent channel falls through to a policy that \
         permits nobody — this is what made the defect silent"
    );
    assert_eq!(
        host.policies.policy_for("addedlater").dm_allowlist,
        Vec::<String>::new(),
        "known-negative: pre-reload, the new channel resolves to the fail-closed default"
    );

    // ---- LEG 2. The operator adds a channel and reloads.
    write_channel(
        &channels,
        "addedlater",
        "U-ADDED-LATER",
        "workspace",
        Some("/srv/jail"),
    );

    let n = host
        .reload_policies()
        .expect("a well-formed channel directory must reload");
    assert_eq!(n, 2, "the reload must pick up both channels");
    assert_eq!(
        host.policies.generation(),
        1,
        "the swap must be observable as a generation bump, not inferred from a count"
    );

    // FACET 1 — the access policy.
    let policy = host.policies.policy_for("addedlater");
    assert_eq!(
        policy.dm_allowlist,
        vec!["U-ADDED-LATER".to_string()],
        "facet 1: the reloaded channel must admit its own allowlisted sender. A failure here \
         is F24-C3-H5 exactly: registered, healthy, 200 — and every message denied"
    );

    // FACET 2 — the tool posture. This is the assertion a policy-only repair
    // fails, and it is why the two maps are one object.
    let scope = host
        .policies
        .scope_for("addedlater")
        .expect("facet 2: the reloaded channel must also carry a tool posture");
    assert_eq!(
        scope.posture,
        ChannelToolPosture::Workspace,
        "facet 2: a policy-only repair leaves this at the dispatcher's Conversational fallback \
         — messages arrive, the naive test goes green, and the channel runs under the wrong \
         permissions"
    );
    assert_eq!(
        scope.workspace_root,
        PathBuf::from("/srv/jail"),
        "facet 2: the configured jail root must survive the reload too"
    );

    // ---- LEG 3. Reloaded posture EQUALS startup posture for the same config.
    //
    // Built through the same production derivation the startup path uses, from
    // the same directory, so the only variable is the lifecycle that produced
    // it — the live control the finding lane ran against a real gateway,
    // reproduced here without one.
    let as_if_at_startup = ChannelPolicyRegistry::from_configs(
        wcore_agent::bootstrap::load_channel_policy_configs().expect("the channel dir parses"),
        Path::new(&workspace),
    )
    .expect("these fixtures are bounded (dm = allowlist over a named sender)");
    for name in ["atstart", "addedlater"] {
        assert_eq!(
            host.policies.policy_for(name),
            as_if_at_startup.policy_for(name),
            "{name}: reloaded policy must EQUAL the policy the same config yields at startup"
        );
        let via_reload = host.policies.scope_for(name);
        let via_startup = as_if_at_startup.scope_for(name);
        assert_eq!(
            via_reload.as_ref().map(|s| (s.posture, &s.workspace_root)),
            via_startup.as_ref().map(|s| (s.posture, &s.workspace_root)),
            "{name}: reloaded POSTURE must EQUAL the posture the same config yields at startup"
        );
    }

    // ---- LEG 4. The unchanged channel survived the reload.
    assert_eq!(
        host.policies.policy_for("atstart").dm_allowlist,
        vec!["U-AT-START".to_string()],
        "the channel that was already there must keep working across a reload"
    );

    // ---- LEG 5. A reload that cannot read the policies REFUSES rather than
    // revoking.
    //
    // The two loaders over `<home>/channels` disagree: the registry SKIPS an
    // unparseable file and registers the rest, while the policy loader stops at
    // the first failure. If the reload swapped in whatever came back, one
    // typo'd file would drop every running channel to the fail-closed default
    // — the same silent-denial defect, arriving from the other direction.
    std::fs::write(channels.join("broken.toml"), "this is not = valid toml [[[")
        .expect("write malformed config");

    let err = host
        .reload_policies()
        .expect_err("a malformed channel config must surface as an error, not an empty reload");
    let msg = err.to_string();
    assert!(
        msg.contains("broken"),
        "the refusal must name the offending file so an operator can fix it; got: {msg}"
    );
    assert_eq!(
        host.policies.generation(),
        1,
        "a refused reload must NOT bump the generation — nothing was swapped"
    );
    assert_eq!(
        host.policies.policy_for("addedlater").dm_allowlist,
        vec!["U-ADDED-LATER".to_string()],
        "a refused reload must KEEP the policies already in effect; revoking them would turn a \
         working gateway into universal denial over an unrelated typo"
    );
    assert_eq!(
        host.policies.policy_for("atstart").dm_allowlist,
        vec!["U-AT-START".to_string()]
    );

    // ---- LEG 6. Fixing the file lets the next reload through, so leg 5 is a
    // refusal and not a wedge.
    std::fs::remove_file(channels.join("broken.toml")).expect("remove malformed config");
    let n = host
        .reload_policies()
        .expect("once the directory is well-formed again the reload must succeed");
    assert_eq!(n, 2);
    assert_eq!(host.policies.generation(), 2);

    // ---- LEG 7. A channel REMOVED from disk loses its policy AND its posture.
    // Without this the repair is a one-way ratchet that can grant authority but
    // never withdraw it.
    std::fs::remove_file(channels.join("addedlater.toml")).expect("remove channel config");
    let n = host.reload_policies().expect("reload after removal");
    assert_eq!(n, 1);
    assert_eq!(
        host.policies.policy_for("addedlater").dm_allowlist,
        Vec::<String>::new(),
        "a removed channel must revert to the fail-closed default"
    );
    assert!(
        host.policies.scope_for("addedlater").is_none(),
        "and must lose its elevated posture with it"
    );
    assert_eq!(
        host.policies.policy_for("atstart").dm_allowlist,
        vec!["U-AT-START".to_string()],
        "positive control: the surviving channel is untouched by the removal"
    );

    unsafe {
        match prev {
            Some(v) => std::env::set_var("WAYLAND_HOME", v),
            None => std::env::remove_var("WAYLAND_HOME"),
        }
    }

    host.shutdown();
}
