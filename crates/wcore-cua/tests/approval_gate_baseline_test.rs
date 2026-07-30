//! 27-C2(c) BASELINE 2 — the approval gate on a computer-use operation, measured in
//! BOTH directions.
//!
//! The criterion clause (`ROADMAP.md:152`, ledger row `27-C2`) asks that CUA surfaces
//! "preserve ... approval ... policy". The ledger records this as having **no baseline at
//! all**, parked behind *"blocked on a display-capable host"*.
//!
//! **That premise is false.** `hetzner-dsm` has `/usr/bin/Xvfb`, `/usr/bin/xvfb-run` and
//! `libXtst.so.6` — XTest being exactly what this crate's X11 backend uses to synthesize
//! input — and `x11` is in this crate's `default` feature set. The only display gate in
//! `backends/linux_x11.rs` is `DISPLAY` being unset (`connect()`), which `xvfb-run` sets.
//!
//! ## Two tests, deliberately
//!
//! 1. `baseline_approval_gate_blocks_dispatch` — **always runs**, no display needed. It
//!    measures that a withheld approval stops the op *upstream of the backend*: the
//!    recording backend's dispatch count stays at 0. Paired with a granted arm where the
//!    count is 1. This exists so the file can never run ZERO tests (LANE-BRIEF §3.2).
//!
//! 2. `baseline_approval_gate_observed_on_real_x11` — behind the crate's own `x11-test`
//!    feature, documented as the "X11 positive-invariance test gate". It does not trust
//!    the Rust return value at all. It reads the **real X11 pointer position back out of
//!    the X server** with `QueryPointer` and asserts on *desktop state*:
//!
//!      * approval WITHHELD  ⇒ `PolicySuspended` **and the pointer has not moved**;
//!      * approval GRANTED   ⇒ `Ok` **and the pointer is at the requested coordinate**.
//!
//!    The granted arm is the load-bearing one. Without it, "the pointer did not move" is
//!    free — a broken instrument, a dead X connection, a wrong screen index and an op
//!    that was never sent all produce it (LANE-BRIEF §3b-i). The granted arm proves the
//!    instrument can see input land, which is what makes the withheld arm mean something.
//!
//! When `x11-test` is on, a missing `DISPLAY` **panics** rather than skipping. A skip is
//! not a pass.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use wcore_cua::backend::{ComputerUseBackend, CuaSession, Platform};
use wcore_cua::error::{CuaError, CuaResult};
use wcore_cua::op::{CuaOp, CuaOpResult};
use wcore_cua::policy::CuaPolicy;
use wcore_cua::tool::CuaTool;

/// Backend that counts how many ops actually reached it. A refusal that is
/// genuinely enforced by the approval gate leaves this at 0.
struct CountingBackend {
    dispatched: AtomicUsize,
    frontmost: Option<String>,
}

impl CountingBackend {
    fn new(frontmost: Option<&str>) -> Self {
        Self {
            dispatched: AtomicUsize::new(0),
            frontmost: frontmost.map(String::from),
        }
    }
    fn count(&self) -> usize {
        self.dispatched.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ComputerUseBackend for CountingBackend {
    fn name(&self) -> &'static str {
        "counting"
    }
    fn platform(&self) -> Platform {
        Platform::Unsupported
    }
    async fn dispatch(&self, _s: &CuaSession, _op: CuaOp) -> CuaResult<CuaOpResult> {
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        Ok(CuaOpResult::Ok)
    }
    async fn frontmost_app(&self) -> CuaResult<Option<String>> {
        Ok(self.frontmost.clone())
    }
}

fn seen_apps_tmp() -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("seen-apps.json");
    (d, p)
}

/// BASELINE 2a — approval gate stops the op BEFORE the backend, in both directions.
/// Runs everywhere; needs no display.
#[tokio::test]
async fn baseline_approval_gate_blocks_dispatch() {
    const APP: &str = "BaselineApp";

    // ── WITHHELD: the app requires per-op HITL approval ──────────────────
    let (_d1, seen1) = seen_apps_tmp();
    let backend = Arc::new(CountingBackend::new(Some(APP)));
    let mut policy = CuaPolicy::permissive().with_seen_apps_path(seen1);
    policy.require_approval_for_app = vec![APP.to_string()];
    let tool = CuaTool::new(backend.clone(), policy);

    let r = tool
        .dispatch(
            CuaSession::for_test("withheld"),
            CuaOp::LeftClick {
                x: 400,
                y: 300,
                button: Default::default(),
                mods: Default::default(),
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    let withheld_suspended = matches!(r, Err(CuaError::PolicySuspended { .. }));
    assert!(
        withheld_suspended,
        "WITHHELD: expected PolicySuspended, got {r:?}"
    );
    assert_eq!(
        backend.count(),
        0,
        "WITHHELD: the op must NOT reach the backend — approval is not advisory"
    );
    println!("EV2A: arm=withheld outcome=PolicySuspended backend_dispatches=0");

    // ── GRANTED: the operator approved, so the app is no longer gated ─────
    // Same op, same app, same backend type. Only the approval state differs.
    let (_d2, seen2) = seen_apps_tmp();
    let backend_ok = Arc::new(CountingBackend::new(Some(APP)));
    let policy_ok = CuaPolicy::permissive().with_seen_apps_path(seen2);
    let tool_ok = CuaTool::new(backend_ok.clone(), policy_ok);
    let r = tool_ok
        .dispatch(
            CuaSession::for_test("granted"),
            CuaOp::LeftClick {
                x: 400,
                y: 300,
                button: Default::default(),
                mods: Default::default(),
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(r.is_ok(), "GRANTED: expected Ok, got {r:?}");
    assert_eq!(
        backend_ok.count(),
        1,
        "GRANTED: the op MUST reach the backend, else the withheld arm proves nothing"
    );
    println!("EV2A: arm=granted outcome=Ok backend_dispatches=1");

    // ── FIRST-TIME-PER-APP gate, both directions on ONE tool ─────────────
    // This is the gate that is ON by serde default (`CuaPolicy::default()`).
    let (_d3, seen3) = seen_apps_tmp();
    let backend_ft = Arc::new(CountingBackend::new(Some("FreshApp")));
    let mut policy_ft = CuaPolicy::permissive().with_seen_apps_path(seen3);
    policy_ft.first_time_per_app_approval = true;
    let tool_ft = CuaTool::new(backend_ft.clone(), policy_ft);

    let op = || CuaOp::MouseMove { x: 111, y: 222 };
    let r = tool_ft
        .dispatch(
            CuaSession::for_test("ft-1"),
            op(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(r, Err(CuaError::PolicySuspended { .. })),
        "FIRST-TIME: expected PolicySuspended on first sight of FreshApp, got {r:?}"
    );
    assert_eq!(
        backend_ft.count(),
        0,
        "FIRST-TIME: first op must not reach the backend"
    );

    // The host's post-approval bookkeeping. This is what "the operator said yes"
    // means to this gate.
    tool_ft.policy().mark_app_seen("FreshApp");

    let r = tool_ft
        .dispatch(
            CuaSession::for_test("ft-2"),
            op(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(
        r.is_ok(),
        "FIRST-TIME: after approval the same op must succeed, got {r:?}"
    );
    assert_eq!(
        backend_ft.count(),
        1,
        "FIRST-TIME: after approval the op must reach the backend exactly once"
    );
    println!(
        "EV2A: arm=first_time_before outcome=PolicySuspended backend_dispatches=0 \
         arm=first_time_after outcome=Ok backend_dispatches=1"
    );

    println!(
        "EV2A-SUMMARY: arms=4 withheld_suspended=2 withheld_backend_dispatches=0 \
         granted_ok=2 granted_backend_dispatches=2 discrimination=PASS"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// BASELINE 2b — the same gate, observed on a REAL X11 server.
// ─────────────────────────────────────────────────────────────────────────

/// Read the real pointer position from the X server. This is the observable —
/// independent of anything the Rust code under test returns.
#[cfg(all(target_os = "linux", feature = "x11-test"))]
fn real_pointer_position() -> (i16, i16) {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt;
    let (conn, screen_idx) = x11rb::rust_connection::RustConnection::connect(None)
        .expect("x11-test: could not connect to $DISPLAY — run under xvfb-run");
    let root = conn.setup().roots[screen_idx].root;
    let reply = conn
        .query_pointer(root)
        .expect("QueryPointer request failed")
        .reply()
        .expect("QueryPointer reply failed");
    (reply.root_x, reply.root_y)
}

/// BASELINE 2b — approval gate measured against real desktop state.
///
/// Gated on the crate's `x11-test` feature (its documented purpose). Serial because
/// it mutates global desktop state — two concurrent tests moving one pointer would
/// make every assertion meaningless.
#[cfg(all(target_os = "linux", feature = "x11-test"))]
#[serial_test::serial]
#[tokio::test]
async fn baseline_approval_gate_observed_on_real_x11() {
    use wcore_cua::backends::linux_x11::LinuxX11Backend;

    // No silent skip. If x11-test is on, a display is REQUIRED.
    assert!(
        std::env::var_os("DISPLAY").is_some(),
        "x11-test is enabled but DISPLAY is unset. This test must not be skipped — \
         run it under `xvfb-run -s '-screen 0 1280x1024x24'`. A skip is not a pass."
    );
    const APP: &str = "BaselineApp";

    // ── STEP 1 — INSTRUMENT LIVENESS (known-positive) ────────────────────
    // A permissive policy must actually move the real pointer. If this fails,
    // every "pointer did not move" assertion below would have been free.
    let backend = Arc::new(LinuxX11Backend::new());
    backend.set_frontmost_for_test(Some(APP.to_string()));
    let (_d0, seen0) = seen_apps_tmp();
    let permissive = CuaPolicy::permissive().with_seen_apps_path(seen0);
    let tool_live = CuaTool::new(backend.clone(), permissive);

    let r = tool_live
        .dispatch(
            CuaSession::for_test("liveness"),
            CuaOp::MouseMove { x: 100, y: 100 },
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(r.is_ok(), "STEP 1: permissive MouseMove must succeed: {r:?}");
    let anchor = real_pointer_position();
    assert_eq!(
        anchor,
        (100, 100),
        "STEP 1 (instrument liveness): XTest+QueryPointer must observe the pointer at \
         the requested coordinate. Got {anchor:?}. Without this the test proves nothing."
    );
    println!("EV2B: step=1-instrument-liveness requested=(100,100) observed={anchor:?} PASS");

    // ── STEP 2 — APPROVAL WITHHELD ⇒ no input reaches the X server ───────
    let backend_w = Arc::new(LinuxX11Backend::new());
    backend_w.set_frontmost_for_test(Some(APP.to_string()));
    let (_d1, seen1) = seen_apps_tmp();
    let mut policy_w = CuaPolicy::permissive().with_seen_apps_path(seen1);
    policy_w.require_approval_for_app = vec![APP.to_string()];
    let tool_w = CuaTool::new(backend_w, policy_w);

    let target = CuaOp::MouseMove { x: 700, y: 500 };
    let r = tool_w
        .dispatch(
            CuaSession::for_test("withheld"),
            target.clone(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(r, Err(CuaError::PolicySuspended { .. })),
        "STEP 2: expected PolicySuspended, got {r:?}"
    );
    let after_withheld = real_pointer_position();
    assert_eq!(
        after_withheld,
        (100, 100),
        "STEP 2: approval was WITHHELD, so NO synthesized input may have reached the X \
         server — the pointer must still be at the anchor. Observed {after_withheld:?}."
    );
    println!(
        "EV2B: step=2-approval-withheld outcome=PolicySuspended requested=(700,500) \
         observed={after_withheld:?} pointer_moved=false PASS"
    );

    // ── STEP 3 — APPROVAL GRANTED ⇒ the SAME op does reach the X server ──
    // Identical op, identical coordinate, identical backend type. The only
    // difference is the approval state. This is the discrimination.
    let backend_g = Arc::new(LinuxX11Backend::new());
    backend_g.set_frontmost_for_test(Some(APP.to_string()));
    let (_d2, seen2) = seen_apps_tmp();
    let policy_g = CuaPolicy::permissive().with_seen_apps_path(seen2);
    let tool_g = CuaTool::new(backend_g, policy_g);

    let r = tool_g
        .dispatch(
            CuaSession::for_test("granted"),
            target.clone(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(r.is_ok(), "STEP 3: expected Ok after approval, got {r:?}");
    let after_granted = real_pointer_position();
    assert_eq!(
        after_granted,
        (700, 500),
        "STEP 3: approval GRANTED, so the op must have reached the X server and moved \
         the pointer to (700,500). Observed {after_granted:?}."
    );
    println!(
        "EV2B: step=3-approval-granted outcome=Ok requested=(700,500) \
         observed={after_granted:?} pointer_moved=true PASS"
    );

    // ── STEP 4/5 — the first-time-per-app gate, same physical observable ──
    let backend_f = Arc::new(LinuxX11Backend::new());
    backend_f.set_frontmost_for_test(Some("FreshApp".to_string()));
    let (_d3, seen3) = seen_apps_tmp();
    let mut policy_f = CuaPolicy::permissive().with_seen_apps_path(seen3);
    policy_f.first_time_per_app_approval = true;
    let tool_f = CuaTool::new(backend_f, policy_f);

    let ft_target = CuaOp::MouseMove { x: 300, y: 250 };
    let r = tool_f
        .dispatch(
            CuaSession::for_test("ft-1"),
            ft_target.clone(),
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(r, Err(CuaError::PolicySuspended { .. })),
        "STEP 4: first sight of FreshApp must Suspend, got {r:?}"
    );
    let after_ft_withheld = real_pointer_position();
    assert_eq!(
        after_ft_withheld,
        (700, 500),
        "STEP 4: the first-time gate must have stopped the input — pointer must still \
         be where STEP 3 left it. Observed {after_ft_withheld:?}."
    );

    tool_f.policy().mark_app_seen("FreshApp");
    let r = tool_f
        .dispatch(
            CuaSession::for_test("ft-2"),
            ft_target,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;
    assert!(r.is_ok(), "STEP 5: after approval the op must succeed, got {r:?}");
    let after_ft_granted = real_pointer_position();
    assert_eq!(
        after_ft_granted,
        (300, 250),
        "STEP 5: after approval the input must reach the X server. Observed \
         {after_ft_granted:?}."
    );
    println!(
        "EV2B: step=4-first-time-withheld observed={after_ft_withheld:?} pointer_moved=false \
         step=5-first-time-granted observed={after_ft_granted:?} pointer_moved=true PASS"
    );

    println!(
        "EV2B-SUMMARY: display={} steps=5 instrument_liveness=PASS \
         withheld_arms=2 withheld_pointer_moved=0 granted_arms=2 granted_pointer_moved=2 \
         discrimination=PASS",
        std::env::var("DISPLAY").unwrap_or_default()
    );
}
