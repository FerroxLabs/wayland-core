//! Wave B4 — live-HTTP round trip against a real Honcho deployment.
//!
//! Disabled in the default build. Enable with
//! `--features live-honcho` and set `HONCHO_API_KEY` (plus optionally
//! `HONCHO_BASE_URL`) to exercise the real wire. When the feature is on
//! but the env var is absent we skip cleanly so CI matrices that flip
//! the feature don't false-fail.

#![cfg(feature = "live-honcho")]

use wayland_honcho::HonchoClient;

#[tokio::test]
async fn live_round_trip_requires_api_key() -> anyhow::Result<()> {
    if std::env::var("HONCHO_API_KEY").is_err() {
        eprintln!("HONCHO_API_KEY unset — skipping live test");
        return Ok(());
    }
    let client = HonchoClient::live_from_env()?;
    let user = format!("test-{}", chrono::Utc::now().timestamp());
    client
        .learn_preference(&user, "tz", "America/New_York")
        .await?;
    let profile = client.recall_user(&user).await?;
    assert_eq!(
        profile.preferences.get("tz").map(|s| s.as_str()),
        Some("America/New_York")
    );
    Ok(())
}
