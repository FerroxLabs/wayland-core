//! 27-C2(c) BASELINE 1 — downloads-root confinement, measured in BOTH directions.
//!
//! The criterion clause (`ROADMAP.md:152`, ledger row `27-C2`) is: *"a browser download
//! must land inside the configured downloads root and must not escape it."* The ledger
//! records this as having **no baseline at all**. This file is that baseline.
//!
//! ## What the pre-existing tests already did, and what was missing
//!
//! `tool.rs`'s inline tests (`download_confined_to_downloads_root`,
//! `download_symlink_escape_is_rejected`, `download_to_dotfile_dest_is_rejected`) assert
//! that the tool returns `is_error` for escaping paths. They use `OkBackend`, which
//! answers `Ok` to everything, so they cannot distinguish
//!
//!   * "the tool refused before dispatch"  from  "the tool dispatched and the backend
//!     happened to do nothing", and they never look at the filesystem.
//!
//! This baseline adds the four things a *measurement* needs:
//!
//!   1. **Provider reach count.** The recording provider counts every op it receives, so
//!      a refusal is proven to be *upstream of dispatch* (count stays 0) rather than a
//!      backend no-op.
//!   2. **Real filesystem effect.** The provider actually WRITES `dest_path` — exactly
//!      what a backend implementing download would do. So "did not escape" is measured
//!      as *no file exists outside the root*, not as an error string.
//!   3. **The discrimination control (LANE-BRIEF §3b-iii "can it pass?").** The *same
//!      literal target path* is REFUSED under root A and ADMITTED under root B (root B
//!      being that path's own parent). A gate that refused everything, or a grep pointed
//!      at the wrong needle, cannot produce that split. This is the control that makes
//!      the refusals mean something.
//!   4. **The "old broken matcher would have missed it" assertion** (§6b-ii, third
//!      assertion). For the symlink arm the test asserts that the escaping path
//!      *lexically* starts with the root — i.e. a naive `starts_with` prefix check would
//!      have ADMITTED it — while the real symlink-resolving gate refuses it.
//!
//! ## Scope limit, stated rather than papered over
//!
//! **No backend in this tree implements `BrowserOp::Download`.** `backends/chromium.rs`
//! returns `Unsupported` explicitly; `backends/camoufox.rs`'s `dispatch` match has no
//! `Download` arm and it falls to the `unsupported` catch-all. So the *"must land inside"*
//! half of the clause cannot be exercised end-to-end through a real browser — not for
//! want of a host, but because the operation does not exist in the product. What IS
//! measurable, and is what actually carries the security property, is the tool-layer
//! gate: `validate_local_path` at `tool.rs:471`, which runs BEFORE any backend dispatch.
//! This file measures that gate against a provider that performs the write the real
//! backend would perform. It does not claim the end-to-end download.

// The URL fixture is a literal public IP, never a hostname. `BrowserPolicy`'s
// DNS resolution gate fails closed on a host that resolves to nothing, so a
// hostname here would make these download-confinement tests depend on the
// runner's resolver — the refusal under test would come from the DNS gate
// rather than the local-path gate. A literal carries its own destination and
// skips resolution, exactly as `dns_resolution_gate_test::PUBLIC_LITERAL` does.

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::json;

use wcore_browser::op::BrowserOp;
use wcore_browser::policy::{BrowserPolicy, PolicyAction};
use wcore_browser::provider::{
    BrowserOpError, BrowserProvider, BrowserSession, OpResult, SessionCtx,
};
use wcore_browser::supervisor::BrowserSupervisor;
use wcore_browser::tool::BrowserTool;
use wcore_tools::Tool;

/// Provider that (a) records every op it is handed and (b) for `Download`
/// actually writes bytes to `dest_path` — the effect a real backend would have.
/// The write is what turns "did not escape" into a filesystem measurement.
#[derive(Default)]
struct RecordingProvider {
    ops: Mutex<Vec<BrowserOp>>,
}

impl RecordingProvider {
    fn op_count(&self) -> usize {
        self.ops.lock().len()
    }

    /// `dest_path` values the provider was actually handed, in order.
    ///
    /// Read only by `baseline_downloads_root_confinement_both_directions`, which is
    /// `#[cfg(unix)]` (it needs `std::os::unix::fs::symlink`). Without the matching
    /// gate this is dead code on Windows and `-D warnings` fails the whole lint job
    /// BEFORE the test step — which is how Windows CI stayed test-blind for six runs.
    #[cfg(unix)]
    fn download_dests(&self) -> Vec<String> {
        self.ops
            .lock()
            .iter()
            .filter_map(|op| match op {
                BrowserOp::Download { dest_path, .. } => Some(dest_path.clone()),
                _ => None,
            })
            .collect()
    }
}

#[async_trait]
impl BrowserProvider for RecordingProvider {
    async fn open_session(
        &self,
        persistent_profile: bool,
    ) -> Result<BrowserSession, BrowserOpError> {
        Ok(BrowserSession {
            ctx: SessionCtx::for_test("recording"),
            persistent_profile,
        })
    }

    async fn close_session(&self, _ctx: &SessionCtx) -> Result<(), BrowserOpError> {
        Ok(())
    }

    async fn dispatch(&self, _ctx: &SessionCtx, op: BrowserOp) -> Result<OpResult, BrowserOpError> {
        self.ops.lock().push(op.clone());
        if let BrowserOp::Download { dest_path, .. } = &op {
            // Perform the write a real download backend would perform. If the
            // gate let an escape through, this is what lands outside the root.
            let p = Path::new(dest_path);
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(p, b"27-C2c-BASELINE-PAYLOAD")
                .map_err(|e| BrowserOpError::Backend(format!("write {dest_path}: {e}")))?;
        }
        Ok(OpResult::Ok)
    }

    fn backend_name(&self) -> &'static str {
        "recording"
    }
}

/// Build a tool whose URL policy ALLOWS the literal-IP fixture host, so any
/// refusal observed is provably the local-path gate and not the URL policy.
///
/// `#[cfg(unix)]` for the same reason as `download_dests` above: its only caller is
/// the symlink-dependent baseline test.
#[cfg(unix)]
fn tool_with_root(provider: Arc<RecordingProvider>, root: &Path) -> BrowserTool {
    BrowserTool::new(
        provider,
        BrowserPolicy::new(PolicyAction::Allow, vec!["93.184.216.34".into()], vec![]),
        Arc::new(BrowserSupervisor::new()),
    )
    .with_downloads_root(root.to_path_buf())
}

fn download_input(dest: &str) -> serde_json::Value {
    json!({
        "op": {
            "kind": "download",
            "url": "https://93.184.216.34/payload.bin",
            "dest_path": dest,
        }
    })
}

/// One escape attempt, measured. Returns `(refused, provider_ops, file_exists)`.
///
/// `#[cfg(unix)]` — see `tool_with_root`.
#[cfg(unix)]
async fn attempt(root: &Path, dest: &str) -> (bool, usize, bool) {
    let provider = Arc::new(RecordingProvider::default());
    let tool = tool_with_root(provider.clone(), root);
    let r = tool.execute(download_input(dest)).await;
    (r.is_error, provider.op_count(), Path::new(dest).exists())
}

/// BASELINE 1. Every number this test reports is asserted, and printed under an
/// `EV1:` prefix so the driver can lift the recorded figures verbatim.
///
/// `#[cfg(unix)]` because the symlink-escape arm (B4) and the naive-prefix arm (C)
/// need `std::os::unix::fs::symlink`. The companion
/// `baseline_default_root_is_fail_closed_pair` below is deliberately NOT gated, so
/// this file never runs ZERO tests on Windows — a suite that exits 0 having run
/// nothing is the vacuity trap in LANE-BRIEF §3.2.
#[cfg(unix)]
#[tokio::test]
async fn baseline_downloads_root_confinement_both_directions() {
    let root_dir = tempfile::tempdir().unwrap();
    let outside_dir = tempfile::tempdir().unwrap();
    let root = root_dir.path();
    let outside = outside_dir.path();

    // ── ARM A (can it PASS?) — a conforming in-root dest_path ────────────
    // This arm is load-bearing: without it, every refusal below is free.
    let provider = Arc::new(RecordingProvider::default());
    let tool = tool_with_root(provider.clone(), root);
    let inside = root.join("report.pdf");
    let r = tool
        .execute(download_input(&inside.to_string_lossy()))
        .await;
    assert!(
        !r.is_error,
        "ARM A: in-root dest must be ADMITTED, got error: {}",
        r.content
    );
    assert_eq!(
        provider.op_count(),
        1,
        "ARM A: the op must actually REACH the provider (else the pass is vacuous)"
    );
    let dests = provider.download_dests();
    assert_eq!(
        dests.len(),
        1,
        "ARM A: exactly one Download reached the provider"
    );
    // The tool normalizes the path in place before dispatch; the provider must
    // therefore receive a path that is itself inside the root.
    let handed = PathBuf::from(&dests[0]);
    let handed_canon = std::fs::canonicalize(&handed).expect("ARM A: handed path must exist");
    let root_canon = std::fs::canonicalize(root).unwrap();
    assert!(
        handed_canon.starts_with(&root_canon),
        "ARM A: provider was handed {handed_canon:?}, which is NOT inside {root_canon:?}"
    );
    assert!(
        inside.exists(),
        "ARM A: the download must have LANDED at {inside:?}"
    );
    let landed_bytes = std::fs::read(&inside).unwrap();
    assert_eq!(
        landed_bytes, b"27-C2c-BASELINE-PAYLOAD",
        "ARM A: landed file content mismatch"
    );
    println!("EV1: arm=A-in-root refused=false provider_ops=1 landed_inside_root=true");

    // ── ARM B (can it FAIL?) — four distinct escape shapes ───────────────
    // Each must: refuse, reach the provider ZERO times, and leave NO file.
    let abs_escape = outside.join("abs-escape.bin");
    let traversal = root.join("..").join("traversal-escape.bin");
    let dotfile = root.join(".ssh").join("authorized_keys");

    // Symlink escape: a directory symlink INSIDE the root pointing OUTSIDE it.
    let link = root.join("innocent");
    std::os::unix::fs::symlink(outside, &link).unwrap();
    let symlink_escape = link.join("loot.bin");

    let arms: Vec<(&str, String)> = vec![
        ("B1-absolute", abs_escape.to_string_lossy().into_owned()),
        ("B2-traversal", traversal.to_string_lossy().into_owned()),
        ("B3-dotfile", dotfile.to_string_lossy().into_owned()),
        ("B4-symlink", symlink_escape.to_string_lossy().into_owned()),
    ];

    let mut refused_count = 0usize;
    let mut total_provider_ops = 0usize;
    let mut files_outside = 0usize;
    for (name, dest) in &arms {
        let (refused, ops, exists) = attempt(root, dest).await;
        assert!(refused, "{name}: escape to {dest} was NOT refused");
        assert_eq!(
            ops, 0,
            "{name}: refusal must be UPSTREAM of dispatch, but the provider saw {ops} op(s)"
        );
        assert!(
            !exists,
            "{name}: a file was created at {dest} — the escape LANDED"
        );
        refused_count += 1;
        total_provider_ops += ops;
        if exists {
            files_outside += 1;
        }
        println!("EV1: arm={name} refused=true provider_ops={ops} file_created=false");
    }
    assert_eq!(refused_count, 4, "all four escape shapes must be refused");
    assert_eq!(
        total_provider_ops, 0,
        "no escaping op may reach the provider"
    );
    assert_eq!(files_outside, 0, "no escaping write may land");

    // ── ARM C — "the naive matcher would have MISSED it" (§6b-ii, 3rd assertion) ──
    // The symlink escape LEXICALLY starts with the root. A prefix check on the
    // unresolved path would have ADMITTED it. Only symlink resolution catches it.
    assert!(
        symlink_escape.starts_with(root),
        "ARM C precondition: {symlink_escape:?} must lexically start with {root:?}, \
         otherwise this arm does not demonstrate what it claims"
    );
    let symlink_real_parent = std::fs::canonicalize(&link).unwrap();
    assert!(
        !symlink_real_parent.starts_with(&root_canon),
        "ARM C precondition: the symlink must really resolve OUTSIDE the root"
    );
    println!(
        "EV1: arm=C-naive-prefix-check lexically_in_root=true really_in_root=false \
         naive_check_verdict=ADMIT real_gate_verdict=REFUSE"
    );

    // ── ARM D — THE DISCRIMINATION CONTROL (can the gate pass on this very path?) ──
    // Same literal target string as B1. Under root=`root` it was refused. Under
    // root=`outside` (its own parent) it must be ADMITTED and must land. This is
    // the "can it pass?" direction for the confinement clause specifically: it
    // proves the refusal was caused by the ROOT BOUNDARY, not by a shape check,
    // a typo'd path, or a gate that refuses unconditionally.
    let same_path = abs_escape.to_string_lossy().into_owned();
    let provider_d = Arc::new(RecordingProvider::default());
    let tool_d = tool_with_root(provider_d.clone(), outside);
    let r_d = tool_d.execute(download_input(&same_path)).await;
    assert!(
        !r_d.is_error,
        "ARM D: {same_path} must be ADMITTED when the root IS its parent, got: {}",
        r_d.content
    );
    assert_eq!(
        provider_d.op_count(),
        1,
        "ARM D: the admitted op must reach the provider"
    );
    assert!(
        Path::new(&same_path).exists(),
        "ARM D: the admitted download must have landed at {same_path}"
    );
    println!(
        "EV1: arm=D-discrimination same_path_refused_under_root_A=true \
         same_path_admitted_under_root_B=true provider_ops=1 landed=true"
    );

    // ── Recorded summary ────────────────────────────────────────────────
    println!(
        "EV1-SUMMARY: escape_shapes_tested=4 escape_shapes_refused=4 \
         provider_ops_on_refusal=0 files_landed_outside_root=0 \
         in_root_admitted=1 in_root_landed=1 discrimination_control=PASS"
    );
}

/// Fail-closed posture: a tool built with `BrowserTool::new` and NEVER told a root
/// still confines, because `new()` installs a default root. Measured as a pair so
/// neither direction is free.
#[tokio::test]
async fn baseline_default_root_is_fail_closed_pair() {
    let provider = Arc::new(RecordingProvider::default());
    let tool = BrowserTool::new(
        provider.clone(),
        BrowserPolicy::new(PolicyAction::Allow, vec!["93.184.216.34".into()], vec![]),
        Arc::new(BrowserSupervisor::new()),
    );

    // NEGATIVE: an absolute path outside the implicit default root is refused,
    // and never reaches the provider.
    let escape = std::env::temp_dir().join("27c2c-default-root-escape.bin");
    let _ = std::fs::remove_file(&escape);
    let r = tool
        .execute(download_input(&escape.to_string_lossy()))
        .await;
    assert!(
        r.is_error,
        "default-root tool must refuse an out-of-root dest: {}",
        r.content
    );
    assert_eq!(
        provider.op_count(),
        0,
        "refusal must be upstream of dispatch"
    );
    assert!(!escape.exists(), "the escaping write must not have landed");

    // POSITIVE: a path INSIDE the implicit default root is admitted and lands —
    // so the refusal above is confinement, not a blanket deny.
    let inside = std::env::temp_dir()
        .join("wayland-downloads")
        .join("27c2c-default-root-ok.bin");
    let _ = std::fs::remove_file(&inside);
    let r = tool
        .execute(download_input(&inside.to_string_lossy()))
        .await;
    assert!(
        !r.is_error,
        "default-root tool must ADMIT an in-root dest: {}",
        r.content
    );
    assert_eq!(
        provider.op_count(),
        1,
        "the admitted op must reach the provider"
    );
    assert!(inside.exists(), "the admitted download must have landed");
    let _ = std::fs::remove_file(&inside);

    println!(
        "EV1-DEFAULTROOT: out_of_root_refused=true out_of_root_provider_ops=0 \
         in_root_admitted=true in_root_provider_ops=1 in_root_landed=true"
    );
}
