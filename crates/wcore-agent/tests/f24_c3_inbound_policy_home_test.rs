//! F24-C3-H1 regression: the inbound ACCESS POLICY must be read from the
//! profile's own channel directory, not from the host user's.
//!
//! ## What broke
//!
//! `AgentBootstrap` registered channel adapters through
//! `wcore_channels_registry::auto_register_from_user_config`, which resolves
//! `$WAYLAND_HOME/channels`, and then loaded the per-channel `[inbound]`
//! policy and tool posture through
//! `ChannelConfigLoader::default_root()`, which joins `$HOME/.wayland/channels`
//! and ignores `WAYLAND_HOME` entirely.
//!
//! Under an isolated profile — every gateway unit, every `--profile`, the
//! desktop host — the second path found nothing, so EVERY channel silently
//! fell back to `InboundPolicy::default()`, which is fail-closed. The adapter
//! registered, started, polled, and reported healthy while denying every
//! inbound message the operator had allowlisted. Measured live on Linux at
//! 15ad7b0e: `inbound denied … reason=sender not in dm allowlist` for a sender
//! named in that very profile's `dm_allowlist`, and the same message admitted
//! the moment a copy of the config was placed under `$HOME/.wayland/channels`.
//!
//! In the other direction, a host whose `$HOME/.wayland/channels` DID hold
//! configs leaked its own allowlists and tool posture — including
//! `tools = "full"` — into a different profile's channels.
//!
//! ## What this test asserts
//!
//! That `load_channel_policy_configs` reads the profile home. It fails if the
//! loader is pointed back at `$HOME`: the profile's permissive allowlist would
//! not be found and the assertion on `dm_allowlist` would see an empty vec.
//!
//! `WAYLAND_HOME` is process-global, so the two directions run inside ONE test
//! function rather than two — nextest would otherwise interleave them.

use std::path::PathBuf;

use wcore_agent::bootstrap::load_channel_policy_configs;
use wcore_channels::dispatch::access::{ChannelToolPosture, DmPolicy};

fn write_channel(dir: &std::path::Path, name: &str, allow: &str, tools: &str) {
    std::fs::create_dir_all(dir).expect("create channels dir");
    std::fs::write(
        dir.join(format!("{name}.toml")),
        format!(
            r#"name = "{name}"
platform = "slack"
enabled = true

[options]
workspace_name = "f24c3"
default_channel_id = "D0"
credential_handle_bot_token = "slack.{name}.bot_token"
credential_handle_signing_secret = "slack.{name}.signing_secret"

[inbound]
dm = "allowlist"
dm_allowlist = ["{allow}"]
group = "disabled"
require_mention = true
tools = "{tools}"
"#
        ),
    )
    .expect("write channel config");
}

#[test]
fn inbound_policy_is_read_from_the_profile_home_not_the_host_home() {
    let host = tempfile::tempdir().expect("host home");
    let profile = tempfile::tempdir().expect("profile home");

    // The host user's own channels — a DIFFERENT channel name, a DIFFERENT
    // allowlist, and the dangerous tool posture. If the loader reads this
    // directory under a profile, the assertions below cannot all hold.
    write_channel(
        &PathBuf::from(host.path()).join(".wayland").join("channels"),
        "hostchannel",
        "U-HOST-USER",
        "full",
    );
    // The profile's own channels.
    write_channel(
        &PathBuf::from(profile.path()).join("channels"),
        "profilechannel",
        "U-PROFILE-USER",
        "conversational",
    );

    // SAFETY: single-threaded test body; no other thread in this integration
    // test binary reads the environment concurrently. Restored below.
    let prev_home = std::env::var_os("HOME");
    let prev_wayland_home = std::env::var_os("WAYLAND_HOME");
    unsafe {
        std::env::set_var("HOME", host.path());
        std::env::set_var("WAYLAND_HOME", profile.path());
    }

    let configs = load_channel_policy_configs();

    unsafe {
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_wayland_home {
            Some(v) => std::env::set_var("WAYLAND_HOME", v),
            None => std::env::remove_var("WAYLAND_HOME"),
        }
    }

    let names: Vec<&str> = configs.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["profilechannel"],
        "the policy loader must see the PROFILE's channels and only those; saw {names:?}"
    );

    let policy = &configs[0].inbound;
    assert_eq!(policy.dm, DmPolicy::Allowlist);
    assert_eq!(
        policy.dm_allowlist,
        vec!["U-PROFILE-USER".to_string()],
        "the profile's own allowlist must be the one in force — an empty vec here is the \
         fail-closed default that denied every allowlisted sender under an isolated profile"
    );
    assert_eq!(
        policy.tools,
        ChannelToolPosture::Conversational,
        "the host user's tool posture must not reach a profile's channels"
    );
}
