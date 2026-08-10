//! DM pairing across TWO holders of one state file.
//!
//! Pairing has two surfaces by construction: the operator mints, lists and
//! revokes from `wayland-core channel pair …` (one short-lived process per
//! invocation), and the gateway decides admission from a long-lived
//! `InboundSubscriber` drain loop (another process entirely). They meet
//! nowhere except `<channels_dir>/pairings/<channel>.toml`.
//!
//! Everything below therefore uses **two separate `PairingBook` handles over
//! one root**, which is exactly what two processes have, and — for the
//! lost-update probes — real child processes.
//!
//! ## Provenance
//!
//! `probe_*` are the three failures the P1 verifier demonstrated
//! (`crates/wcore-channels/tests/verifier_cache.rs`, since deleted). They are
//! reproduced here verbatim in behaviour, including the assertion shapes that
//! produced the reported `left`/`right` output, so the fix is measured against
//! the original attack rather than a friendlier restatement of it.
//!
//! `variant_*` are further attacks in the same class — shared mutable state
//! over one file — written against the fix rather than against the bug.

use std::path::{Path, PathBuf};
use std::process::Command;

use wcore_channels::{
    AccessDecision, ChatType, DEFAULT_CODE_TTL_MS, DmPolicy, InboundPolicy, IncomingMessage,
    PairingBook, decide_access_paired,
};

const CHANNEL: &str = "slack";

/// Child-process role. Present only in a process this file spawned.
const ROLE_ENV: &str = "WLP1_XPROC_ROLE";
const ROOT_ENV: &str = "WLP1_XPROC_ROOT";
const SENDER_ENV: &str = "WLP1_XPROC_SENDER";
const CODE_ENV: &str = "WLP1_XPROC_CODE";
const GO_ENV: &str = "WLP1_XPROC_GO";

/// Exit code a child uses for "the operation was refused".
const EXIT_REFUSED: i32 = 7;

/// The test a child process is told to run. Arbitrary — role dispatch happens
/// before any test body — but it must name a test that exists in this file.
const CHILD_TEST: &str = "variant_concurrent_processes_lose_no_minted_code";

fn dm(sender: &str, text: &str) -> IncomingMessage {
    let mut m = IncomingMessage::new("m1", "conv1", "Alice", text, 0);
    m.sender_id = sender.into();
    m.chat_type = ChatType::Direct;
    m
}

fn pairing_policy() -> InboundPolicy {
    InboundPolicy {
        dm: DmPolicy::Pairing,
        // Deliberately wide open: pairing ignores the allowlist, so nothing
        // below can be admitted by it.
        dm_allowlist: vec!["*".into()],
        ..Default::default()
    }
}

/// One admission decision made by the RUNNING host, through the production
/// gate — not by poking the book directly.
fn host_decides(book: &mut PairingBook, sender: &str, text: &str, now_ms: i64) -> AccessDecision {
    decide_access_paired(CHANNEL, &dm(sender, text), &pairing_policy(), book, now_ms)
}

// ---------------------------------------------------------------------------
// The three probes the verifier used
// ---------------------------------------------------------------------------

/// (a) A code minted AFTER the gateway has seen any prior DM must still admit.
///
/// The original failure: `PairingBook::state()` loaded a channel's file on
/// first touch only, and nothing invalidated it — so the operator's mint
/// landed in a file the running host had already stopped reading. The verifier
/// reported `left: Deny { reason: "pairing required" }, right: Allow`.
#[test]
fn probe_a_code_minted_after_first_touch_admits_on_the_running_host() {
    if run_child_role() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("pairings");

    // The gateway process, long-lived from here on.
    let mut gateway = PairingBook::open(&root);
    // Ordinary unpaired traffic arrives first. This is the "first touch".
    assert!(matches!(
        host_decides(&mut gateway, "u1", "hello", 1_000),
        AccessDecision::Deny { .. }
    ));

    // A SECOND process: `wayland-core channel pair mint slack`.
    let code = PairingBook::open(&root)
        .mint(CHANNEL, 1_100, DEFAULT_CODE_TTL_MS)
        .unwrap();

    // The person DMs that code to the running gateway.
    assert_eq!(
        host_decides(&mut gateway, "u1", &code, 1_200),
        AccessDecision::Allow,
        "a code minted by the operator must admit on the RUNNING host"
    );
}

/// (b) `channel pair revoke` must deny the sender on the running host.
///
/// The original failure: the CLI printed `slack: revoked u1` and the gateway
/// kept admitting them until the process restarted.
#[test]
fn probe_operator_revoke_takes_effect_on_the_running_host() {
    if run_child_role() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("pairings");

    let code = PairingBook::open(&root)
        .mint(CHANNEL, 1_000, DEFAULT_CODE_TTL_MS)
        .unwrap();

    let mut gateway = PairingBook::open(&root);
    assert_eq!(
        host_decides(&mut gateway, "u1", &code, 1_100),
        AccessDecision::Allow
    );

    // A fresh operator process — every CLI invocation is one.
    let revoked = PairingBook::open(&root).unpair(CHANNEL, "u1").unwrap();
    assert!(revoked, "the CLI reports success, so it must BE success");

    match host_decides(&mut gateway, "u1", "still here", 1_200) {
        AccessDecision::Deny { .. } => {}
        AccessDecision::Allow => panic!("revoked sender must be denied by the RUNNING host"),
    }
}

/// (c) The running host must not clobber operator state.
///
/// The original failure: the host wrote its whole cached snapshot back over
/// the file, destroying a code minted in between. The verifier reported
/// `live_code_count left: 0, right: 1`.
#[test]
fn probe_running_host_does_not_clobber_operator_state() {
    if run_child_role() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("pairings");

    // Operator mints the first code.
    let first = PairingBook::open(&root)
        .mint(CHANNEL, 1_000, DEFAULT_CODE_TTL_MS)
        .unwrap();

    // The gateway touches the channel before the second mint happens.
    let mut gateway = PairingBook::open(&root);
    assert!(matches!(
        host_decides(&mut gateway, "u1", "hello", 1_100),
        AccessDecision::Deny { .. }
    ));

    // Operator mints a second code for a second person.
    let second = PairingBook::open(&root)
        .mint(CHANNEL, 1_200, DEFAULT_CODE_TTL_MS)
        .unwrap();

    // The first person redeems. The host writes — and must write only its own
    // change, not its stale view of the whole file.
    assert_eq!(
        host_decides(&mut gateway, "u1", &first, 1_300),
        AccessDecision::Allow
    );

    let live_code_count = PairingBook::open(&root)
        .live_code_count(CHANNEL, 1_400)
        .unwrap();
    assert_eq!(
        live_code_count, 1,
        "the operator's second code must survive the host's write"
    );

    // …and it must still work for the person it was minted for.
    assert_eq!(
        host_decides(&mut gateway, "u2", &second, 1_500),
        AccessDecision::Allow
    );
}

// ---------------------------------------------------------------------------
// Further attacks in the same class
// ---------------------------------------------------------------------------

/// Real OS processes, concurrent mints. Every code must survive.
///
/// This is the lost-update the in-process probes only approximate: N children
/// each perform an independent read-modify-write of one file at the same
/// moment. Deterministic in the green direction (a serialized RMW can only
/// produce N); probabilistic in the red direction, where the loser count
/// depends on scheduling.
#[test]
fn variant_concurrent_processes_lose_no_minted_code() {
    if run_child_role() {
        return;
    }
    const CHILDREN: usize = 12;

    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("pairings");
    let go = tmp.path().join("go");

    let kids: Vec<_> = (0..CHILDREN)
        .map(|_| spawn_child("mint", &root, &go, &[]))
        .collect();
    release(&go);
    for kid in kids {
        assert_child_ok(kid, "mint");
    }

    assert_eq!(
        PairingBook::open(&root)
            .live_code_count(CHANNEL, 1_000)
            .unwrap(),
        CHILDREN,
        "every concurrently minted code must be on disk"
    );
}

/// Real OS processes, one code, two senders. Exactly one may be admitted.
///
/// Single-use is only single-use if the check and the burn are one atomic
/// step across processes. Two gateway processes (a restart overlap, or the
/// F24-CL lease briefly held twice) both racing the same code must not both
/// win.
#[test]
fn variant_two_processes_cannot_both_redeem_one_code() {
    if run_child_role() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("pairings");
    let go = tmp.path().join("go");

    let code = PairingBook::open(&root)
        .mint(CHANNEL, 1_000, DEFAULT_CODE_TTL_MS)
        .unwrap();

    let kids: Vec<_> = ["racer-a", "racer-b"]
        .iter()
        .map(|sender| {
            spawn_child(
                "redeem",
                &root,
                &go,
                &[(SENDER_ENV, sender), (CODE_ENV, code.as_str())],
            )
        })
        .collect();
    release(&go);

    let mut admitted = 0usize;
    for kid in kids {
        if child_outcome(kid, "redeem") {
            admitted += 1;
        }
    }
    assert_eq!(
        admitted, 1,
        "a single-use code must admit exactly one sender"
    );

    let paired = PairingBook::open(&root).paired_senders(CHANNEL).unwrap();
    assert_eq!(
        paired.len(),
        1,
        "exactly one sender may end up paired, saw {paired:?}"
    );
}

/// Revocation by deleting the state: the running host must lose the pairing.
///
/// An operator who deletes `<channels_dir>/pairings` is performing the
/// bluntest revocation the product offers. A host holding a cached snapshot
/// keeps admitting everyone it had already admitted, which makes the only
/// panic button inert.
#[test]
fn variant_deleting_the_state_denies_on_the_running_host() {
    if run_child_role() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("pairings");

    let code = PairingBook::open(&root)
        .mint(CHANNEL, 1_000, DEFAULT_CODE_TTL_MS)
        .unwrap();
    let mut gateway = PairingBook::open(&root);
    assert_eq!(
        host_decides(&mut gateway, "u1", &code, 1_100),
        AccessDecision::Allow
    );

    std::fs::remove_dir_all(&root).unwrap();

    match host_decides(&mut gateway, "u1", "am I still in", 1_200) {
        AccessDecision::Deny { .. } => {}
        AccessDecision::Allow => panic!("deleted pairing state must not keep admitting"),
    }
}

/// The read direction: a pairing completed by the gateway is visible to the
/// operator's very next `channel pair list`, with no restart.
#[test]
fn variant_operator_sees_a_pairing_the_host_just_completed() {
    if run_child_role() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("pairings");

    // The operator process exists FIRST and has already read the channel —
    // the same first-touch shape that broke the host.
    let mut operator = PairingBook::open(&root);
    assert!(operator.paired_senders(CHANNEL).unwrap().is_empty());
    let code = operator.mint(CHANNEL, 1_000, DEFAULT_CODE_TTL_MS).unwrap();

    let mut gateway = PairingBook::open(&root);
    assert_eq!(
        host_decides(&mut gateway, "u1", &code, 1_100),
        AccessDecision::Allow
    );

    let paired = operator.paired_senders(CHANNEL).unwrap();
    assert_eq!(
        paired
            .iter()
            .map(|p| p.sender_id.as_str())
            .collect::<Vec<_>>(),
        vec!["u1"],
        "the operator must see a pairing the host completed after they started"
    );
    assert_eq!(operator.live_code_count(CHANNEL, 1_200).unwrap(), 0);
}

/// `revoke-codes` from the operator must invalidate a code the host has
/// already seen the channel for but not yet redeemed.
#[test]
fn variant_revoke_codes_kills_a_code_the_host_already_knows_about() {
    if run_child_role() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("pairings");

    let code = PairingBook::open(&root)
        .mint(CHANNEL, 1_000, DEFAULT_CODE_TTL_MS)
        .unwrap();

    // The host reads the channel — under the old cache this is the moment the
    // live code became frozen into its private copy.
    let mut gateway = PairingBook::open(&root);
    assert!(matches!(
        host_decides(&mut gateway, "u1", "hello", 1_100),
        AccessDecision::Deny { .. }
    ));

    assert_eq!(
        PairingBook::open(&root).revoke_codes(CHANNEL).unwrap(),
        1,
        "the operator revokes the leaked code"
    );

    match host_decides(&mut gateway, "u1", &code, 1_200) {
        AccessDecision::Deny { .. } => {}
        AccessDecision::Allow => panic!("a revoked code must not admit on the RUNNING host"),
    }
}

/// An operator reading while hosts are writing never sees a torn file.
///
/// `channel pair list` runs at an arbitrary moment. If a reader could observe
/// a half-published file it would report a parse error — or worse, under a
/// fail-open read, an empty state. Neither may happen while writers churn.
#[test]
fn variant_a_reader_never_observes_a_half_written_state() {
    if run_child_role() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("pairings");
    let go = tmp.path().join("go");

    let writers: Vec<_> = (0..6)
        .map(|_| spawn_child("mint", &root, &go, &[]))
        .collect();
    release(&go);

    // Hammer the read path for as long as the writers are running.
    let mut operator = PairingBook::open(&root);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(750);
    let mut reads = 0usize;
    while std::time::Instant::now() < deadline {
        operator
            .live_code_count(CHANNEL, 1_000)
            .expect("a reader must never observe a torn or unreadable state");
        reads += 1;
    }
    assert!(reads > 0, "the reader loop never ran");

    for kid in writers {
        assert_child_ok(kid, "mint");
    }
    assert_eq!(operator.live_code_count(CHANNEL, 1_000).unwrap(), 6);
}

/// A hostile channel name cannot escape the pairing root — including via the
/// sibling lock file the cross-process serialization needs.
#[test]
fn variant_channel_name_cannot_escape_the_pairing_root() {
    if run_child_role() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("pairings");
    let mut book = PairingBook::open(&root);

    for bad in ["../escape", "a/b", "", ".hidden", "sl ack", "..", "a\\b"] {
        assert!(
            book.mint(bad, 1_000, DEFAULT_CODE_TTL_MS).is_err(),
            "channel name {bad:?} must be refused"
        );
        assert!(
            book.live_code_count(bad, 1_000).is_err(),
            "channel name {bad:?} must be refused on read too"
        );
    }
    // Nothing was created anywhere outside the root, including no lock files.
    let strays: Vec<PathBuf> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p != &root)
        .collect();
    assert!(
        strays.is_empty(),
        "hostile channel names created {strays:?}"
    );
}

// ---------------------------------------------------------------------------
// Child-process harness
// ---------------------------------------------------------------------------

/// If this process was spawned as a child, perform its role and exit.
/// Returns `true` when it handled a role (the caller must return immediately).
fn run_child_role() -> bool {
    let Ok(role) = std::env::var(ROLE_ENV) else {
        return false;
    };
    let root = PathBuf::from(std::env::var(ROOT_ENV).expect("child root"));
    let go = PathBuf::from(std::env::var(GO_ENV).expect("child gate"));
    // Every child blocks on the same file so the read-modify-writes actually
    // overlap instead of running in spawn order.
    while !go.exists() {
        std::hint::spin_loop();
    }

    let mut book = PairingBook::open(&root);
    let ok = match role.as_str() {
        "mint" => book.mint(CHANNEL, 1_000, DEFAULT_CODE_TTL_MS).is_ok(),
        "redeem" => {
            let sender = std::env::var(SENDER_ENV).expect("child sender");
            let code = std::env::var(CODE_ENV).expect("child code");
            book.admit(CHANNEL, &dm(&sender, &code), 1_100)
        }
        other => panic!("unknown child role {other:?}"),
    };
    std::process::exit(if ok { 0 } else { EXIT_REFUSED });
}

fn spawn_child(role: &str, root: &Path, go: &Path, extra: &[(&str, &str)]) -> std::process::Child {
    let mut cmd = Command::new(std::env::current_exe().expect("test binary path"));
    // Run exactly ONE test in the child, and which one does not matter: every
    // test in this file dispatches to `run_child_role` before touching its own
    // body, so the child performs its role and exits from there.
    cmd.args([CHILD_TEST, "--exact"])
        .env(ROLE_ENV, role)
        .env(ROOT_ENV, root)
        .env(GO_ENV, go);
    for (k, v) in extra {
        cmd.env(k, v);
    }
    cmd.spawn().expect("spawn child")
}

/// Open the gate all children are spinning on.
fn release(go: &Path) {
    std::fs::create_dir_all(go.parent().unwrap()).unwrap();
    std::fs::write(go, b"go").unwrap();
}

fn child_outcome(kid: std::process::Child, role: &str) -> bool {
    let status = kid.wait_with_output().expect("child wait");
    match status.status.code() {
        Some(0) => true,
        Some(code) if code == EXIT_REFUSED => false,
        other => panic!(
            "{role} child exited {other:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        ),
    }
}

fn assert_child_ok(kid: std::process::Child, role: &str) {
    assert!(child_outcome(kid, role), "{role} child was refused");
}
