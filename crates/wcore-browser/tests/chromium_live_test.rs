//! W10 — chromium live e2e. Closes debt-register A.1: chromiumoxide live
//! tests don't run in CI because runners don't ship Chromium pre-installed.
//!
//! Gated by the `browser-live-tests` feature so a default `cargo nextest run`
//! on a dev box does NOT try to launch Chromium. The dedicated `browser-live`
//! CI job (`.github/workflows/ci.yml`) installs `chromium-browser` via apt
//! and runs only this file.
//!
//! Scope of this test:
//!   * Spawn a real Chromium via `ChromiumBackend` (chromiumoxide CDP).
//!   * Navigate to a `file://` URL pointing at `tests/fixtures/hello.html`.
//!   * Assert page state (URL/title round-trip) and that the `h1#greeting`
//!     element exists in the live DOM via `WaitFor`.
//!   * Tear down via the backend's `close_session` + drop.
//!
//! Why not assert against `Snapshot` (ARIA tree)? `ChromiumBackend::dispatch`
//! returns `Unsupported` for `Snapshot` in chromiumoxide v0.7 — see the
//! comment on `BrowserOp::Snapshot` in `backends/chromium.rs`. Camoufox is
//! the ARIA-tree-first backend; this test exercises the chromium fallback
//! path on its supported surface (Navigate / GetState / WaitFor).

#![cfg(feature = "browser-live-tests")]

use std::path::PathBuf;

use wcore_browser::backends::ChromiumBackend;
use wcore_browser::op::BrowserOp;
use wcore_browser::provider::{BrowserProvider, OpResult};

/// Path to the static fixture, resolved from `CARGO_MANIFEST_DIR` so the
/// `file://` URL works regardless of where the test binary was invoked from.
fn fixture_url() -> String {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture = manifest_dir
        .join("tests")
        .join("fixtures")
        .join("hello.html");
    assert!(
        fixture.exists(),
        "fixture missing at {} — build artefact problem?",
        fixture.display()
    );
    // file:// + absolute path. On Unix the canonical form is `file:///abs/path`.
    // The chromiumoxide goto path accepts this directly.
    format!("file://{}", fixture.display())
}

/// Pick the chromium executable. Order:
///   1. `WCORE_CHROMIUM_PATH` env var if set + exists.
///   2. `chromium-browser` (Ubuntu apt package — the CI install target).
///   3. `chromium`, `google-chrome`, `chrome` (other Linuxes / Macs).
///
/// Returns the first match; if none exist, falls back to letting chromiumoxide
/// auto-detect by passing `None` to `ChromiumBackend::new`.
fn pick_chromium() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("WCORE_CHROMIUM_PATH") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    let candidates = [
        "/usr/bin/chromium-browser",
        "/usr/bin/chromium",
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/snap/bin/chromium",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    for c in candidates {
        let pb = PathBuf::from(c);
        if pb.exists() {
            return Some(pb);
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chromium_live_navigates_to_file_url_and_finds_heading() {
    let url = fixture_url();
    let backend = match pick_chromium() {
        Some(p) => ChromiumBackend::with_executable(p),
        None => ChromiumBackend::new(),
    };

    let session = backend.open_session(false).await.expect(
        "chromium open_session: is Chromium installed? \
                 (CI: `apt-get install -y chromium-browser`; \
                 local: set WCORE_CHROMIUM_PATH to your binary)",
    );

    // Navigate to the file:// fixture.
    let nav = backend
        .dispatch(
            &session.ctx,
            BrowserOp::Navigate {
                url: url.clone(),
                wait_until_loaded: true,
            },
        )
        .await
        .expect("chromium Navigate failed");
    assert!(
        matches!(nav, OpResult::Ok),
        "Navigate should return Ok, got {nav:?}"
    );

    // Round-trip page state. Title is the `<title>` element in the fixture.
    let state = backend
        .dispatch(&session.ctx, BrowserOp::GetState {})
        .await
        .expect("chromium GetState failed");
    match &state {
        OpResult::State {
            url: live_url,
            title,
        } => {
            assert!(
                live_url.starts_with("file://") && live_url.ends_with("hello.html"),
                "unexpected live URL: {live_url}"
            );
            assert_eq!(title, "W10 Live", "fixture <title> round-trip failed");
        }
        other => panic!("expected State, got {other:?}"),
    }

    // Verify the heading exists in the live DOM. `WaitFor` polls
    // `find_element` against the CDP-resolved DOM — proves chromium parsed
    // the HTML and the element is reachable. This is the chromium-backend
    // equivalent of "ARIA tree contains the heading text" — `Snapshot` is
    // `Unsupported` in chromiumoxide v0.7, so we assert on DOM presence.
    let wait = backend
        .dispatch(
            &session.ctx,
            BrowserOp::WaitFor {
                selector: "h1#greeting".into(),
                timeout_ms: 5_000,
            },
        )
        .await
        .expect("chromium WaitFor(h1#greeting) failed — heading not in DOM");
    assert!(
        matches!(wait, OpResult::Ok),
        "WaitFor should return Ok, got {wait:?}"
    );

    backend
        .close_session(&session.ctx)
        .await
        .expect("chromium close_session failed");
}
