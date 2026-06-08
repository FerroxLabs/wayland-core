//! Wave B4 — mock-mode round-trip tests for `HonchoClient`.
//!
//! Mock is the default flavour (no network, deterministic). The live
//! flavour lives in `tests/live_test.rs` and is gated by the
//! `live-honcho` feature.

use wayland_honcho::{HonchoClient, UserProfile};

#[tokio::test]
async fn mock_round_trips_preference() {
    let client = HonchoClient::mock();
    client
        .learn_preference("user-1", "color", "blue")
        .await
        .unwrap();
    let profile: UserProfile = client.recall_user("user-1").await.unwrap();
    assert_eq!(
        profile.preferences.get("color").map(|s| s.as_str()),
        Some("blue")
    );
}

#[tokio::test]
async fn mock_recall_unknown_user_returns_empty_profile() {
    let client = HonchoClient::mock();
    let profile = client.recall_user("never-seen").await.unwrap();
    assert!(profile.preferences.is_empty());
    assert_eq!(profile.user_id, "never-seen");
}
