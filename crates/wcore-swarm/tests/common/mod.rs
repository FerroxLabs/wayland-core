//! Honest skip gate for the delegated-dispatch swarm suites.
//!
//! # The problem this exists for
//!
//! `Swarm::dispatch` refuses to run a worker unless a sandbox backend can own
//! the complete descendant tree (`owns_descendants_hard`). On a stock macOS CI
//! runner NOTHING satisfies that: the `sandbox_exec` primary is not a
//! hard-containment backend, and the qualified Docker fallback needs both the
//! `wcore-sandbox/live-docker` feature (default-OFF) and a live daemon. The
//! error says so itself:
//!
//! ```text
//! sandbox backend sandbox_exec cannot own descendants that escape a process
//! group; select Docker for delegated Swarm execution on this host; qualified
//! Docker fallback is unavailable on this macOS host: docker backend disabled
//! (feature `live-docker` off)
//! ```
//!
//! Eight tests therefore had NO reachable pass state on that leg — a gate that
//! cannot pass, which is exactly as worthless as one that cannot fail. This
//! module makes them skip HONESTLY where the backend is absent while leaving
//! every assertion intact and every run on a qualifying host unchanged.
//!
//! # Why the file is the load-bearing channel, not the print
//!
//! `cargo nextest` CAPTURES the output of a PASSING test. A skip that only
//! prints is therefore invisible in the very run that matters, so a
//! print-only skip is indistinguishable from a pass in CI. The prints below are
//! kept for `--no-capture` and plain `cargo test`, but the RECORD is a file the
//! CI step reads back. Same reasoning, and the same idiom, as
//! `crates/wcore-agent/tests/anvil_forge_transaction.rs::record_loud_skip`.
//!
//! # Why a skip can never hide a regression on the leg that proves the contract
//!
//! [`REQUIRE_DELEGATED_BACKEND`]`=1` converts the skip into a PANIC. The Linux
//! containerized CI leg arms it, so if bwrap ever stops qualifying there the
//! suite goes RED instead of quietly skipping into a green run. Skipping is
//! only ever available where the host genuinely cannot host the guarantee.

// Each integration-test binary that says `mod common;` compiles this file
// separately, and none of them uses every item — `SKIP_LEDGER` in particular is
// consumed by the CI step, not by Rust. Without this the unused-in-THIS-binary
// items would trip `dead_code`, and CI runs `clippy --all-targets -D warnings`.
// Scoped to this helper module only.
#![allow(dead_code)]

/// Set to `1` to make an unavailable delegated backend a hard test failure
/// instead of a recorded skip. Armed on the Linux containerized CI leg.
pub const REQUIRE_DELEGATED_BACKEND: &str = "WCORE_REQUIRE_DELEGATED_BACKEND";

/// Filename of the skip ledger inside the cargo target directory.
pub const SKIP_LEDGER: &str = "swarm-delegated-skips.txt";

/// Resolve the real cargo target directory.
///
/// A relative `"target"` would be WRONG: the test's working directory is the
/// crate root (`crates/wcore-swarm`), not the workspace root, so it would write
/// to a `crates/wcore-swarm/target/` that does not exist. `CARGO_TARGET_TMPDIR`
/// is an absolute path INSIDE the real target dir, so the nearest ancestor named
/// `target` is the target dir itself. (Lifted verbatim in shape from
/// `anvil_forge_transaction.rs`, where writing the wrong path once made a
/// "counted" skip count nothing.)
fn target_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("CARGO_TARGET_DIR") {
        return std::path::PathBuf::from(d);
    }
    let mut p: &std::path::Path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    loop {
        if p.file_name().is_some_and(|n| n == "target") {
            return p.to_path_buf();
        }
        match p.parent() {
            Some(parent) => p = parent,
            None => return std::path::PathBuf::from("target"),
        }
    }
}

/// Emit the skip loudly AND record it where a CI step can aggregate it.
///
/// Panics rather than degrading if the record cannot be written: if the count
/// cannot be persisted then nothing downstream can tell a recorded skip from an
/// unrecorded one, and "recorded" becomes a claim rather than a fact. The panic
/// is also the only way that failure becomes visible at all, since a panic
/// un-captures the output nextest would otherwise swallow.
fn record_loud_skip(test: &str, reason: &str) {
    let line = format!("WCORE_SWARM_DELEGATED_SKIP test={test} reason={reason}");
    println!("!!!! SKIPPED (NOT PASSED): {line}");
    eprintln!("!!!! SKIPPED (NOT PASSED): {line}");
    eprintln!(
        "!!!! this test proves a delegated-dispatch guarantee and it did NOT run. \
         Set {REQUIRE_DELEGATED_BACKEND}=1 to make this a hard failure."
    );

    let dir = target_dir();
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("cannot create {} to record the skip: {e}", dir.display()));
    let path = dir.join(SKIP_LEDGER);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap_or_else(|e| {
            panic!(
                "cannot record the delegated skip at {}: {e}",
                path.display()
            )
        });
    // ONE `write_all` of a record that already ends in '\n', NOT `writeln!`.
    // MEASURED, not theorised: `writeln!` on an unbuffered `File` issues the
    // text and the newline as two separate `write` syscalls, and these eight
    // tests run in four concurrent nextest processes appending to one file. The
    // first version of this helper produced a torn ledger on a real run —
    // `...read denialWCORE_SWARM_DELEGATED_SKIP test=...` spliced onto one line,
    // so eight records rendered as seven lines. Anything that counted lines
    // would have undercounted the skips it exists to make visible. A single
    // `write_all` under O_APPEND is atomic for a record this far below PIPE_BUF.
    use std::io::Write as _;
    f.write_all(format!("{line}\n").as_bytes())
        .unwrap_or_else(|e| panic!("cannot write the delegated skip to {}: {e}", path.display()));
}

/// `true` when the caller must SKIP because no backend can host delegated
/// dispatch on this host; `false` when a real backend qualified and the test
/// body must run in full.
///
/// The probe is `wcore_swarm::dispatch::delegated_backend_qualification`, which
/// calls the SAME `select_delegated_backend` that `dispatch` itself calls — so
/// this cannot qualify a host that dispatch would then refuse, and it cannot
/// skip on a host where dispatch would have worked.
pub async fn skip_without_delegated_backend(test: &str) -> bool {
    match wcore_swarm::dispatch::delegated_backend_qualification().await {
        Ok(backend) => {
            // Printed so a --no-capture run shows WHICH backend carried the
            // proof, rather than leaving "it passed" ambiguous between
            // "genuinely contained" and "qualified out".
            println!("delegated backend qualified for {test}: {backend}");
            false
        }
        Err(reason) => {
            if std::env::var(REQUIRE_DELEGATED_BACKEND).is_ok_and(|v| v == "1") {
                panic!(
                    "{REQUIRE_DELEGATED_BACKEND}=1 but no delegated sandbox backend qualifies: \
                     {reason}. This leg exists to PROVE the delegated-dispatch guarantee; \
                     skipping {test} here would make the gate unfalsifiable."
                );
            }
            record_loud_skip(test, &reason);
            true
        }
    }
}
