//! The child-ATTRIBUTION corpus harness — Phase 21, plan 21-04.
//!
//! ## What this file is
//!
//! One corpus, defined once as data in `child_attribution_corpus/cases.rs`,
//! executed in TWO modes for every entry: in process against the real seams,
//! and LIVE against the real shipped `wayland-core` binary. It answers Phase
//! 21's Success Criterion 2 — *nested reservation, refund, escalation,
//! approval, cancellation and result delivery stay attributable to the correct
//! parent and session* — which is a different property from Criterion 1 and
//! needs its own proof. A system can be perfectly non-amplifying and still
//! misattribute.
//!
//! ## Construction rules that are structural, not conventional
//!
//! * **At least two siblings, always.** `every_case_runs_at_least_two_siblings`
//!   fails the table if any entry drops below two. One child under one parent
//!   attributes correctly by accident.
//! * **At least one crash-and-restart case.** `the_corpus_carries_a_crash_and_restart_case`
//!   fails the table if none does. Attribution proved only inside one process
//!   lifetime is attribution proved in the easy case only.
//! * **Every entry runs in both modes.** `assert_completeness` fails an entry
//!   whose recorded execution set is short, so writing an in-process-only case
//!   is structurally impossible rather than merely discouraged.
//!
//! ## What is asserted, and what is recorded
//!
//! This plan PROVES and states a verdict; it REPAIRS nothing. The split is
//! stated here so nobody mistakes a recorded red for an ignored one:
//!
//! * **Asserted** — the corpus's own integrity, and the two failures that mean
//!   the corpus itself is lying: an execution stamped with the wrong event, a
//!   live verdict taken from a run that never proved its mode, and an
//!   in-process CORRECT standing against a live MISATTRIBUTED. That last one is
//!   the class this codebase has already shipped once, where the plumbing is
//!   right and the user still sees the wrong thing.
//! * **Recorded** — every MISATTRIBUTED and NOT-OBSERVABLE verdict, carried
//!   into `21-04-ATTRIBUTION-RESULTS.md` with its severity. Failing on them
//!   here would not make them fixed; it would only make the proof
//!   uncommittable, and the severity-classified list is what the phase verdict
//!   consumes.
//!
//! No assertion anywhere names an error string, error kind, error variant or
//! numeric status. Every verdict is on which actor the event landed on.
//!
//! ## Layout
//!
//! One `#[test]` per lifecycle event, so every case id appears in the run
//! transcript and a case that did not execute cannot be mistaken for one that
//! passed. `cargo nextest` runs each in its own process, which is why the
//! completeness invariant is asserted per entry rather than through shared
//! mutable state that would not survive the process model.

#[path = "child_attribution_corpus/cases.rs"]
mod cases;
#[path = "child_attribution_corpus/live.rs"]
mod live;
#[path = "support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use cases::{AttributionCase, LIFECYCLE_EVENTS, LifecycleEvent};
use live::{Attribution, LiveEvidence, LiveTransport};

use wcore_agent::approval::{ApprovalBridge, ApprovalOutcome, ApprovalRequest};
use wcore_agent::budget_authority::{BudgetAuthorityCoordinator, BudgetAuthoritySeed};
use wcore_agent::durable_child::DurableChildStore;
use wcore_agent::session_journal::{
    BudgetWallClockAuthority, SessionEvent, SessionJournal, state_payload_digest,
};
use wcore_budget::execution::ExecutionBudget;
use wcore_budget::tracker::{BudgetCap, BudgetExtensionError, BudgetTracker};
use wcore_types::spawner::{
    ChildDeliveryState, ChildDeliveryTarget, ChildDesiredState, ChildId, ChildOrigin, ChildParent,
    ChildPolicySnapshot, ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace,
    ChildWorkspaceMode, DURABLE_CHILD_SCHEMA_VERSION, DurableChildRecord, DurableChildResult,
    DurableChildStatus, DurableChildTransition,
};

/// The two sibling actors every case runs with. Distinct identities, so a
/// misattribution has somewhere wrong to land.
const SIBLING_A: &str = "corpus-attr-parent/child-alpha";
const SIBLING_B: &str = "corpus-attr-parent/child-beta";
/// The two sibling GRANDCHILDREN, for the cases whose attribution question is
/// about which ancestor an event rolls up to rather than which peer owns it.
const GRANDCHILD_A: &str = "corpus-attr-parent/child-alpha/grandchild-a1";
const GRANDCHILD_B: &str = "corpus-attr-parent/child-alpha/grandchild-a2";

/// The platform this run records for, as the results table spells it.
fn platform() -> &'static str {
    if cfg!(windows) { "windows" } else { "linux" }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread tokio runtime for the in-process probes")
}

/// One (case, mode) execution.
#[derive(Debug, Clone)]
struct Execution {
    event: LifecycleEvent,
    /// `in-process`, or the live transport's label.
    mode: String,
    attribution: Attribution,
    /// The observation that distinguished correct attribution from a
    /// misattribution — or, when it could not, what was and was not seen.
    detail: String,
    live: Option<LiveEvidence>,
}

// ===========================================================================
// The in-process drivers — the real seams, not stand-ins
// ===========================================================================

/// A tracker with headroom far above anything the probes reserve, so a refusal
/// is never mistaken for a misattribution. `per_user_daily_usd` stays unset:
/// `BudgetTracker::reserve` fails closed when a daily cap is active, and a
/// fail-closed refusal would make every downstream observation an absence.
fn roomy_tracker() -> BudgetTracker {
    BudgetTracker::new(
        BudgetCap::builder()
            .per_session_tokens(1_000_000)
            .per_session_usd(1_000.0)
            .build(),
    )
}

fn usd_eq(left: f64, right: f64) -> bool {
    (left - right).abs() < 1e-9
}

/// Compare one actor's `(tokens, usd)` books against an expectation without
/// making a rounding artefact look like a misattribution.
fn totals_eq(left: (u64, f64), right: (u64, f64)) -> bool {
    left.0 == right.0 && usd_eq(left.1, right.1)
}

/// Seed a journal's canonical imported session baseline.
///
/// The reducer requires the import to be the journal's first event and to carry
/// the journal's own session id, a matching `schema_version` and an array of
/// object messages. Without it a durable budget authority refuses to bind at
/// all, so this is the precondition for reaching the crash-survival seam rather
/// than a fixture convenience.
fn import_session_baseline(journal: &SessionJournal, session_id: &str) -> Result<(), String> {
    let session = serde_json::json!({
        "id": session_id,
        "schema_version": 1,
        "messages": [],
    });
    let session_digest = state_payload_digest(&session).map_err(|error| error.to_string())?;
    journal
        .append(SessionEvent::SessionImported {
            source_schema_version: 1,
            session_digest,
            session,
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// RESERVATION. Two siblings reserve DIFFERENT amounts against the same parent
/// envelope, so the two reservations are distinguishable by size as well as by
/// owner: a tracker that credited both to one actor, or swapped them, produces
/// a different pair of numbers than a correct one.
fn probe_reservation() -> (Attribution, String) {
    let mut tracker = roomy_tracker();
    if let Err(error) = tracker.reserve(SIBLING_A, 100, 0.10) {
        return (
            Attribution::NotObservable,
            format!(
                "sibling A could not reserve at all, so there was nothing to attribute: {error}"
            ),
        );
    }
    if let Err(error) = tracker.reserve(SIBLING_B, 250, 0.25) {
        return (
            Attribution::NotObservable,
            format!(
                "sibling B could not reserve at all, so there was no second place a misattribution could land: {error}"
            ),
        );
    }
    let (a_tokens, a_usd) = tracker.reserved_totals(SIBLING_A);
    let (b_tokens, b_usd) = tracker.reserved_totals(SIBLING_B);
    let detail = format!(
        "sibling A reserved 100 tokens / $0.10 and its books read {a_tokens} tokens / \
         ${a_usd:.4}; sibling B reserved 250 / $0.25 and its books read {b_tokens} tokens / \
         ${b_usd:.4}"
    );
    if a_tokens == 100 && usd_eq(a_usd, 0.10) && b_tokens == 250 && usd_eq(b_usd, 0.25) {
        (
            Attribution::Correct,
            format!(
                "{detail} — each reservation landed on the sibling that made it and on no other"
            ),
        )
    } else {
        (Attribution::Misattributed, detail)
    }
}

/// REFUND, ACROSS A CRASH AND RESTART.
///
/// Two siblings reserve through the journal-coupled authority coordinator; the
/// coordinator is dropped, which is what a process death looks like to the
/// journal; a second coordinator is bound over the SAME journal file; and only
/// then is one sibling's reservation refunded. `budget_authority.rs` exists
/// precisely because runtime budget mutation is only useful if the same
/// authority survives a crash, so a refund proved without a restart proves the
/// easy half.
///
/// ## What the first version of this probe got wrong
///
/// It carried the pre-crash `BudgetReservation` handles across the restart and
/// asked the rebound tracker to `release` one of them, then read
/// `reserved_totals`. Both reads came back empty and it recorded
/// NOT-OBSERVABLE, escalated as F21-04-02 — a suspected durability defect.
///
/// It was the wrong meter, not a missing reservation. A restart deliberately
/// does not carry in-flight reservations forward as refundable handles.
/// `BudgetAuthorityCoordinator::bind` settles every one of them as it binds,
/// and the disposition depends on the evidence the journal carries:
///
/// * A reservation bound to a provider dispatch is decided on that dispatch's
///   own attempt records — charged at its admitted maximum if a physical send
///   is journalled, refunded only if one provably never started
///   (`budget_authority.rs::reconcile_dispatch_bound_reservations`).
/// * An UNBOUND reservation — which is what this probe makes, through
///   `BudgetTracker::reserve` — has no evidence either way about whether the
///   provider was paid, so it is settled CONSERVATIVELY at its admitted
///   maximum (`restore` →
///   `BudgetTracker::reconcile_restored_reservations_conservatively`).
///
/// Either way `reserved_totals` reads zero afterwards because the settlement
/// completed, and `release` returns false because the handle was consumed by
/// it. On this probe's path the money is not returned by a crash; it is
/// charged. Reading only the reserved meter saw the reservation leave and
/// concluded it had never arrived.
///
/// The same disproof is driven against the real binary across a real `SIGKILL`
/// by `f14_sigkill_recovery::
/// sigkill_mid_dispatch_charges_the_surviving_reservation_instead_of_refunding_it`.
///
/// ## What this probe measures now
///
/// Three legs, all on the attribution question and none on a mechanism:
///
/// 1. **The reservations are still on the siblings that made them after the
///    crash.** The durable authority is reloaded out of the journal FILE,
///    before anything rebinds, and each sibling's reserved books are read off
///    it. This is the leg that distinguishes "not persisted" from "persisted,
///    then reconciled".
/// 2. **The restart posts each sibling's own reservation to its own charged
///    books.** A restart that credited one sibling with the other's admitted
///    maximum is exactly the misattribution this case exists to catch, and it
///    is now visible because the charged meter is read as well as the reserved
///    one.
/// 3. **A refund on the crash-survivor authority still lands on one sibling
///    only.** A reserves again on the rebound coordinator and is refunded; B is
///    not touched, on either meter.
fn probe_refund_across_restart() -> (Attribution, String) {
    // Bound to `_temp` rather than `_`: the directory must outlive both
    // coordinator bindings, and `_` would drop it immediately.
    let Ok(_temp) = tempfile::tempdir() else {
        return (
            Attribution::NotObservable,
            "no temporary directory was available to hold the session journal".to_owned(),
        );
    };
    let path = _temp.path().join("corpus-attribution.journal");
    let session = "corpus-attr-session";
    let seed = BudgetAuthoritySeed {
        provider_caps: BudgetCap::builder()
            .per_session_tokens(1_000_000)
            .per_session_usd(1_000.0)
            .build(),
        preserve_committed_session_extensions: false,
        execution_policy: ExecutionBudget::default(),
        wall_clock: BudgetWallClockAuthority::ActiveRuntime,
        process_cleanup_proof: None,
        daily_authority: None,
    };

    {
        let journal = match SessionJournal::open(&path, session) {
            Ok(journal) => journal,
            Err(error) => {
                return (
                    Attribution::NotObservable,
                    format!("the session journal could not be opened: {error}"),
                );
            }
        };
        // A durable budget authority refuses to bind against a journal with no
        // canonical imported session baseline (`budget_authority.rs:182`), and
        // the import must be the journal's FIRST event
        // (`session_journal/reducer.rs:1662`). Seeding it is what makes the
        // crash-and-restart leg reach the durable seam at all.
        if let Err(error) = import_session_baseline(&journal, session) {
            return (
                Attribution::NotObservable,
                format!("the journal's canonical session baseline could not be imported: {error}"),
            );
        }
        let mut coordinator =
            match BudgetAuthorityCoordinator::bind(seed.config(Some(journal), session)) {
                Ok(coordinator) => coordinator,
                Err(error) => {
                    return (
                        Attribution::NotObservable,
                        format!("the durable budget authority could not be bound: {error}"),
                    );
                }
            };
        let outcome = coordinator.transaction(|mutation| {
            let tracker = mutation.provider_tracker();
            let a = tracker.reserve(SIBLING_A, 100, 0.10).is_ok();
            let b = tracker.reserve(SIBLING_B, 250, 0.25).is_ok();
            a && b
        });
        match outcome {
            Ok(true) => {}
            Ok(false) => {
                return (
                    Attribution::NotObservable,
                    "one of the two siblings could not reserve, so the two-sibling topology never \
                     existed before the restart"
                        .to_owned(),
                );
            }
            Err(error) => {
                return (
                    Attribution::NotObservable,
                    format!("the reservation transaction did not commit: {error}"),
                );
            }
        }
        // `coordinator` and its journal handle drop here. That is the crash.
    }

    // LEG 1. The crash has happened and nothing has rebound yet. Read the
    // durable authority straight out of the journal file and rebuild a tracker
    // from it: both reservations are still there, on the siblings that made
    // them, at the amounts they made them for.
    let persisted = match durable_reserved_totals(&path, session, &seed.provider_caps) {
        Ok(totals) => totals,
        Err(reason) => return (Attribution::NotObservable, reason),
    };
    if !totals_eq(persisted.alpha, (100, 0.10)) || !totals_eq(persisted.beta, (250, 0.25)) {
        // Not a misattribution: if the reservations are not in the journal
        // there is no refund for the restart to attribute to anyone. Recorded
        // as the durability fact it would be.
        return (
            Attribution::NotObservable,
            format!(
                "the journal survived the crash carrying sibling A's reserved books as {:?} and \
                 sibling B's as {:?}, against the 100 tokens / $0.1000 and 250 tokens / $0.2500 \
                 they respectively reserved, so there was no surviving reservation to attribute a \
                 refund to",
                persisted.alpha, persisted.beta
            ),
        );
    }

    // The restart: a second authority bound over the same journal file. Binding
    // is itself the restart reconciliation — every reservation recovered from
    // the dead process is settled at its admitted maximum, against the sibling
    // that owns it.
    let rebound_journal = match SessionJournal::open(&path, session) {
        Ok(journal) => journal,
        Err(error) => {
            return (
                Attribution::NotObservable,
                format!("the session journal could not be reopened after the restart: {error}"),
            );
        }
    };
    let mut rebound =
        match BudgetAuthorityCoordinator::bind(seed.config(Some(rebound_journal), session)) {
            Ok(coordinator) => coordinator,
            Err(error) => {
                return (
                    Attribution::NotObservable,
                    format!("the budget authority could not be rebound after the restart: {error}"),
                );
            }
        };

    // LEGS 2 and 3, in one transaction on the crash-survivor authority: read
    // where the restart posted each sibling's reservation, then refund a fresh
    // reservation belonging to sibling A alone.
    let observed = rebound.transaction(|mutation| {
        let tracker = mutation.provider_tracker();
        let settled_a = tracker.session_totals(SIBLING_A);
        let settled_b = tracker.session_totals(SIBLING_B);
        let carried_a = tracker.reserved_totals(SIBLING_A);
        let carried_b = tracker.reserved_totals(SIBLING_B);
        let refunded = match tracker.reserve(SIBLING_A, 40, 0.04) {
            Ok(reservation) => {
                let held_a = tracker.reserved_totals(SIBLING_A);
                let held_b = tracker.reserved_totals(SIBLING_B);
                let released = tracker.release(reservation);
                Some((
                    released,
                    held_a,
                    held_b,
                    tracker.reserved_totals(SIBLING_A),
                    tracker.reserved_totals(SIBLING_B),
                    tracker.session_totals(SIBLING_A),
                    tracker.session_totals(SIBLING_B),
                ))
            }
            Err(_) => None,
        };
        (settled_a, settled_b, carried_a, carried_b, refunded)
    });
    let (settled_a, settled_b, carried_a, carried_b, refunded) = match observed {
        Ok(values) => values,
        Err(error) => {
            return (
                Attribution::NotObservable,
                format!("the refund transaction did not commit after the restart: {error}"),
            );
        }
    };
    let Some((released, held_a, held_b, after_a, after_b, charged_a, charged_b)) = refunded else {
        return (
            Attribution::NotObservable,
            "the crash-survivor authority refused sibling A a fresh reservation, so there was no \
             refund to attribute"
                .to_owned(),
        );
    };

    let detail = format!(
        "the crash left sibling A's reserved books at {:?} and sibling B's at {:?} in the journal; \
         the restart posted {settled_a:?} to sibling A's charged books and {settled_b:?} to \
         sibling B's, carrying {carried_a:?} and {carried_b:?} forward as still-reserved; sibling \
         A then reserved again ({held_a:?} against sibling B's {held_b:?}) and the refund \
         reported {released}, leaving reserved {after_a:?} / {after_b:?} and charged \
         {charged_a:?} / {charged_b:?}",
        persisted.alpha, persisted.beta
    );

    // The restart must charge each sibling ITS OWN admitted maximum and carry
    // nothing forward as reserved.
    let restart_posted_per_sibling = totals_eq(settled_a, (100, 0.10))
        && totals_eq(settled_b, (250, 0.25))
        && totals_eq(carried_a, (0, 0.0))
        && totals_eq(carried_b, (0, 0.0));
    // The refund must reduce only the sibling that made the reservation, and
    // must not disturb either sibling's charged books.
    let refund_landed_on_one_sibling = released
        && totals_eq(held_a, (40, 0.04))
        && totals_eq(held_b, (0, 0.0))
        && totals_eq(after_a, (0, 0.0))
        && totals_eq(after_b, (0, 0.0))
        && totals_eq(charged_a, (100, 0.10))
        && totals_eq(charged_b, (250, 0.25));

    if restart_posted_per_sibling && refund_landed_on_one_sibling {
        (
            Attribution::Correct,
            format!(
                "{detail} — the reservations survived the crash on the siblings that made them, \
                 the restart charged each sibling only its own, and the refund reduced only the \
                 sibling whose reservation it released"
            ),
        )
    } else {
        (Attribution::Misattributed, detail)
    }
}

/// Reload the durable budget authority from a journal FILE and report each
/// sibling's still-reserved books.
///
/// Deliberately reads the file rather than a live coordinator: a coordinator
/// reconciles restored reservations as it binds, so only the file can answer
/// whether the reservations were persisted at all.
fn durable_reserved_totals(
    path: &std::path::Path,
    session: &str,
    caps: &BudgetCap,
) -> Result<SurvivingReservedBooks, String> {
    let journal = SessionJournal::open(path, session).map_err(|error| {
        format!("the session journal could not be reopened after the crash: {error}")
    })?;
    let authority = journal
        .state()
        .map_err(|error| format!("the crashed session's journal did not reduce: {error}"))?
        .budget_authority
        .ok_or_else(|| {
            "the crashed process committed no durable budget authority, so there was nothing for \
             the restart to attribute"
                .to_owned()
        })?;
    let tracker = BudgetTracker::from_snapshot_with_current_caps(
        authority.provider_tracker.clone(),
        caps.clone(),
    )
    .map_err(|error| format!("the durable provider authority did not reload: {error}"))?;
    Ok(SurvivingReservedBooks {
        alpha: tracker.reserved_totals(SIBLING_A),
        beta: tracker.reserved_totals(SIBLING_B),
    })
}

/// Each sibling's still-reserved `(tokens, usd)` as the crashed process left
/// them in the journal.
#[derive(Debug, Clone, Copy)]
struct SurvivingReservedBooks {
    alpha: (u64, f64),
    beta: (u64, f64),
}

/// ESCALATION, three generations deep. Two sibling GRANDCHILDREN under one
/// child: one exhausts its envelope and receives an operator-authorised
/// extension; the other asked for nothing. An escalation's whole question is
/// which descendant the extra headroom lands on, which is why this case is not
/// run at two levels.
fn probe_escalation() -> (Attribution, String) {
    let mut tracker = BudgetTracker::new(
        BudgetCap::builder()
            .per_session_tokens(1_000)
            .per_session_usd(1.0)
            .build(),
    );
    let base_a = tracker.effective_session_limits(GRANDCHILD_A);
    let base_b = tracker.effective_session_limits(GRANDCHILD_B);

    // Precondition: an unexhausted actor cannot be extended. Establishing this
    // first is what makes the later observation on sibling B meaningful.
    let unexhausted = tracker.extend_session(GRANDCHILD_B, 500, 1.0);
    if !matches!(unexhausted, Err(BudgetExtensionError::NoExhaustedBudget)) {
        return (
            Attribution::NotObservable,
            format!(
                "an actor that had asked for nothing was extendable before any exhaustion \
                 ({unexhausted:?}), so this seam does not gate escalation on the escalating actor \
                 at all and there is no attribution to read"
            ),
        );
    }

    // Drive grandchild A to exhaustion through the ADMISSION path.
    //
    // Measured, not assumed: `charge` records usage and returns the cap error
    // but does NOT add the session to the blocked set — only `reserve_turn` and
    // `settle_turn` do (`tracker.rs:910/921/932/951` and `:1038-1083`). An
    // earlier iteration of this probe charged past the cap and then found the
    // extension refused for want of an exhausted budget, which was the harness
    // driving the wrong seam rather than the product refusing an escalation.
    let mut exhausted = false;
    for _ in 0..8 {
        if tracker.reserve(GRANDCHILD_A, 400, 0.40).is_err() {
            exhausted = true;
            break;
        }
    }
    if !exhausted {
        return (
            Attribution::NotObservable,
            "grandchild A never exhausted its envelope, so no escalation was raised".to_owned(),
        );
    }

    let granted = tracker.extend_session(GRANDCHILD_A, 500, 1.0);
    let after_a = tracker.effective_session_limits(GRANDCHILD_A);
    let after_b = tracker.effective_session_limits(GRANDCHILD_B);
    let sibling_still_gated = tracker.extend_session(GRANDCHILD_B, 500, 1.0);
    let detail = format!(
        "grandchild A exhausted its envelope and its extension reported {granted:?}; A's \
         effective limits were {base_a:?} before and {after_a:?} after; the sibling \
         grandchild's were {base_b:?} before and {after_b:?} after, and a second extension \
         attempt on the sibling reported {sibling_still_gated:?}"
    );
    if granted.is_err() {
        return (
            Attribution::NotObservable,
            format!(
                "{detail} — the escalation itself did not take effect, so nothing was attributed"
            ),
        );
    }
    let sibling_untouched = after_b == base_b
        && matches!(
            sibling_still_gated,
            Err(BudgetExtensionError::NoExhaustedBudget)
        );
    if sibling_untouched && after_a != base_a {
        (
            Attribution::Correct,
            format!(
                "{detail} — the escalation widened only the descendant that exhausted its \
                 envelope and left the sibling neither widened nor unblocked"
            ),
        )
    } else {
        (Attribution::Misattributed, detail)
    }
}

/// APPROVAL. Two siblings each raise an approval under the same parent bridge
/// and exactly one is answered. With one pending approval an answer resolves it
/// by construction; with two, an answer that resolves the wrong one — or both —
/// is visible.
fn probe_approval() -> (Attribution, String) {
    let rt = runtime();
    rt.block_on(async {
        let bridge = ApprovalBridge::new();
        let request = |call_id: &str| ApprovalRequest {
            call_id: call_id.to_owned(),
            reason: "the corpus asks the sibling to mutate".to_owned(),
            context: "child-attribution corpus".to_owned(),
        };
        let (_token_a, mut rx_a) = bridge
            .request_with_id(SIBLING_A.to_owned(), request(SIBLING_A))
            .await;
        let (_token_b, mut rx_b) = bridge
            .request_with_id(SIBLING_B.to_owned(), request(SIBLING_B))
            .await;
        let pending_before = bridge.pending_count().await;
        if pending_before < 2 {
            return (
                Attribution::NotObservable,
                format!(
                    "only {pending_before} approval(s) were outstanding after two siblings each \
                     raised one, so the two-sibling topology never existed"
                ),
            );
        }

        // Answer exactly ONE sibling.
        let resolved = bridge
            .resolve_by_correlation(
                SIBLING_A,
                ApprovalOutcome {
                    approved: true,
                    modifications: None,
                    cancellation: None,
                },
            )
            .await;
        let a_answer = rx_a.try_recv();
        let b_answer = rx_b.try_recv();
        let pending_after = bridge.pending_count().await;
        let detail = format!(
            "two siblings raised approvals; answering sibling A's reported {resolved}; sibling \
             A's outcome is {a_answer:?} and sibling B's is {b_answer:?}; {pending_before} \
             approval(s) were outstanding before the answer and {pending_after} after"
        );
        let a_got_it = a_answer.map(|outcome| outcome.approved).unwrap_or(false);
        let b_still_outstanding = b_answer.is_err() && pending_after == pending_before - 1;
        if resolved && a_got_it && b_still_outstanding {
            (
                Attribution::Correct,
                format!(
                    "{detail} — the answer resolved only the sibling it was given for and left \
                     the other outstanding"
                ),
            )
        } else {
            (Attribution::Misattributed, detail)
        }
    })
}

// ---------------------------------------------------------------------------
// Durable-child fixtures, shared by the cancellation and delivery probes
// ---------------------------------------------------------------------------

fn digest(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn sibling_record(
    id: &str,
    parent_child_id: Option<&str>,
    delivery_target: Option<ChildDeliveryTarget>,
) -> DurableChildRecord {
    DurableChildRecord {
        schema_version: DURABLE_CHILD_SCHEMA_VERSION,
        declaration_id: format!("corpus-attr-declare-{id}"),
        child_id: ChildId::new(id).expect("a valid child id"),
        parent: ChildParent {
            session_id: "corpus-attr-session".into(),
            // Left unset deliberately. The journal reducer resolves a declared
            // turn id against turns it has actually seen, and this corpus
            // declares its children directly rather than through a live turn;
            // the attribution being measured is the parent/child edge, which
            // `parent_child_id` and `parent_call_id` carry.
            turn_id: None,
            parent_child_id: parent_child_id
                .map(|value| ChildId::new(value).expect("a valid parent child id")),
            workflow_run_id: None,
            graph_node_id: None,
            parent_call_id: Some(format!("spawn:{id}")),
        },
        origin: ChildOrigin::Spawn,
        request: ChildRequestEvidence::redacted(digest('a')),
        policy_snapshot: ChildPolicySnapshot {
            contract_version: "effective-execution-policy/v1".into(),
            exact_digest: digest('b'),
            posture: "standard".into(),
            approvals: "ask".into(),
            sandbox: "workspace-write".into(),
            source: "session-effective-policy".into(),
            managed_floor_active: true,
            dangerous_activation_id_digest: None,
        },
        provider: Some("anthropic".into()),
        model: Some("corpus-model".into()),
        workspace: ChildWorkspace {
            mode: ChildWorkspaceMode::Isolated,
            workspace_id: format!("corpus-attr-workspace-{id}"),
        },
        status: DurableChildStatus::Prepared,
        desired_state: ChildDesiredState::Run,
        recovery: ChildRecoveryState::Clean,
        revision: 0,
        timestamps: ChildTimestamps {
            created_at_unix_ms: 100,
            updated_at_unix_ms: 100,
            queued_at_unix_ms: None,
            started_at_unix_ms: None,
            terminal_at_unix_ms: None,
        },
        result: None,
        delivery_state: if delivery_target.is_some() {
            ChildDeliveryState::Pending
        } else {
            ChildDeliveryState::NotRequired
        },
        delivery_target,
        attempt: 1,
        retry_of: None,
        applied_events: BTreeMap::new(),
    }
}

fn corpus_result() -> DurableChildResult {
    DurableChildResult {
        exact_digest: digest('e'),
        turns: 1,
        input_tokens: 10,
        output_tokens: 10,
        artifact_digests: Vec::new(),
    }
}

/// Open a journal-backed durable child store in a fresh temporary directory.
fn durable_store() -> Result<(tempfile::TempDir, DurableChildStore), String> {
    let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
    let path = temp.path().join("corpus-attribution-children.journal");
    let journal =
        SessionJournal::open(&path, "corpus-attr-session").map_err(|error| error.to_string())?;
    Ok((temp, DurableChildStore::new(journal)))
}

/// CANCELLATION. Two siblings are running and exactly one is cancelled.
fn probe_cancellation() -> (Attribution, String) {
    let (_temp, store) = match durable_store() {
        Ok(pair) => pair,
        Err(reason) => {
            return (
                Attribution::NotObservable,
                format!("the durable child store could not be opened: {reason}"),
            );
        }
    };
    let alpha = ChildId::new("corpus-attr-alpha").expect("a valid child id");
    let beta = ChildId::new("corpus-attr-beta").expect("a valid child id");
    for id in [alpha.as_str(), beta.as_str()] {
        if let Err(error) = store.declare(sibling_record(id, None, None)) {
            return (
                Attribution::NotObservable,
                format!("sibling {id} could not be declared: {error}"),
            );
        }
    }
    // Bring both siblings to Running, so the cancellation has two live places
    // it could land.
    for (id, prefix) in [(&alpha, "a"), (&beta, "b")] {
        for (event, revision, at, transition) in [
            (
                format!("{prefix}-enqueue"),
                0,
                101,
                DurableChildTransition::Enqueue,
            ),
            (
                format!("{prefix}-start"),
                1,
                102,
                DurableChildTransition::Start,
            ),
        ] {
            if let Err(error) = store.transition(id.clone(), event, revision, at, transition) {
                return (
                    Attribution::NotObservable,
                    format!("sibling {id} could not be brought to Running: {error}"),
                );
            }
        }
    }

    // Cancel exactly ONE sibling.
    if let Err(error) = store.transition(
        alpha.clone(),
        "a-request-cancel",
        2,
        103,
        DurableChildTransition::RequestCancel,
    ) {
        return (
            Attribution::NotObservable,
            format!("the cancellation could not be requested at all: {error}"),
        );
    }

    let (Ok(Some(after_alpha)), Ok(Some(after_beta))) =
        (store.inspect(&alpha), store.inspect(&beta))
    else {
        return (
            Attribution::NotObservable,
            "one of the two sibling records could not be read back after the cancellation"
                .to_owned(),
        );
    };
    let detail = format!(
        "after cancelling only sibling alpha: alpha desired_state {:?} / status {:?}; beta \
         desired_state {:?} / status {:?}",
        after_alpha.desired_state, after_alpha.status, after_beta.desired_state, after_beta.status
    );
    if after_alpha.desired_state == ChildDesiredState::Cancel
        && after_beta.desired_state == ChildDesiredState::Run
        && after_beta.status == DurableChildStatus::Running
    {
        (
            Attribution::Correct,
            format!("{detail} — the cancellation stopped only the sibling it was directed at"),
        )
    } else {
        (Attribution::Misattributed, detail)
    }
}

/// RESULT DELIVERY, three generations deep, across the two
/// `ChildDeliveryTarget` variants reachable without a live turn.
///
/// MEASURED, and recorded as a coverage limit rather than worked around:
/// `ChildDeliveryTarget::ParentTurn` is refused at declaration unless
/// `parent.turn_id` names a turn the journal has actually seen
/// (`session_journal/reducer.rs:1597`), and this corpus declares its children
/// directly rather than through a live turn. `ParentTurn` is therefore left
/// UNEXERCISED in process and said so, rather than faked with a synthetic turn
/// that would prove attribution against a fixture instead of against the
/// product.
///
/// The topology is otherwise the hardest one this seam admits. Two child
/// siblings deliver to the same `SessionOutbox`, and two GRANDCHILD
/// siblings under one of them deliver to the SAME target,
/// `ParentChild { child_id }`. Sharing a destination is the point: delivering
/// one grandchild must not mark the other delivered even though both are bound
/// for the same place, which a store that keyed delivery by target rather than
/// by producer would get wrong and a two-generation corpus could never see.
///
/// A correction to this plan's own reading of the vocabulary, recorded because
/// the misreading is easy to repeat: `ParentChild { child_id }` names the
/// record's OWN parent child, and `validate_declaration` requires
/// `parent.parent_child_id == Some(child_id)`. It is not a pointer to some other
/// child a result is being handed to.
fn probe_delivery() -> (Attribution, String) {
    let (_temp, store) = match durable_store() {
        Ok(pair) => pair,
        Err(reason) => {
            return (
                Attribution::NotObservable,
                format!("the durable child store could not be opened: {reason}"),
            );
        }
    };
    let alpha = ChildId::new("corpus-attr-alpha").expect("a valid child id");
    let beta = ChildId::new("corpus-attr-beta").expect("a valid child id");
    let alpha_one = ChildId::new("corpus-attr-alpha-one").expect("a valid child id");
    let alpha_two = ChildId::new("corpus-attr-alpha-two").expect("a valid child id");
    let to_parent_child = ChildDeliveryTarget::ParentChild {
        child_id: alpha.clone(),
    };

    let declarations = [
        sibling_record(
            alpha.as_str(),
            None,
            Some(ChildDeliveryTarget::SessionOutbox),
        ),
        sibling_record(
            beta.as_str(),
            None,
            Some(ChildDeliveryTarget::SessionOutbox),
        ),
        sibling_record(
            alpha_one.as_str(),
            Some(alpha.as_str()),
            Some(to_parent_child.clone()),
        ),
        sibling_record(
            alpha_two.as_str(),
            Some(alpha.as_str()),
            Some(to_parent_child.clone()),
        ),
    ];
    for record in declarations {
        let id = record.child_id.clone();
        if let Err(error) = store.declare(record) {
            return (
                Attribution::NotObservable,
                format!("child {id} could not be declared: {error}"),
            );
        }
    }

    // All four run to a result; only grandchild alpha-one is then delivered.
    for (id, prefix) in [
        (&alpha, "a"),
        (&beta, "b"),
        (&alpha_one, "a1"),
        (&alpha_two, "a2"),
    ] {
        for (event, revision, at, transition) in [
            (
                format!("{prefix}-enqueue"),
                0,
                201,
                DurableChildTransition::Enqueue,
            ),
            (
                format!("{prefix}-start"),
                1,
                202,
                DurableChildTransition::Start,
            ),
            (
                format!("{prefix}-succeed"),
                2,
                203,
                DurableChildTransition::Succeed {
                    result: corpus_result(),
                },
            ),
        ] {
            if let Err(error) = store.transition(id.clone(), event, revision, at, transition) {
                return (
                    Attribution::NotObservable,
                    format!("child {id} could not be brought to a result: {error}"),
                );
            }
        }
    }
    for (event, revision, at, transition) in [
        (
            "a1-delivery-start",
            3,
            204,
            DurableChildTransition::DeliveryStarted,
        ),
        (
            "a1-delivery-done",
            4,
            205,
            DurableChildTransition::DeliveryDelivered {
                receipt_digest: digest('f'),
            },
        ),
    ] {
        if let Err(error) = store.transition(alpha_one.clone(), event, revision, at, transition) {
            return (
                Attribution::NotObservable,
                format!("grandchild alpha-one's result could not be delivered: {error}"),
            );
        }
    }

    let read = |id: &ChildId| store.inspect(id).ok().flatten();
    let (Some(after_alpha), Some(after_beta), Some(after_one), Some(after_two)) = (
        read(&alpha),
        read(&beta),
        read(&alpha_one),
        read(&alpha_two),
    ) else {
        return (
            Attribution::NotObservable,
            "one of the four records could not be read back after the delivery".to_owned(),
        );
    };
    let detail = format!(
        "after delivering only grandchild alpha-one: alpha-one {:?} to {:?}; its sibling \
         grandchild alpha-two {:?} to {:?}; child alpha {:?} to {:?}; child beta {:?} to {:?}",
        after_one.delivery_state,
        after_one.delivery_target,
        after_two.delivery_state,
        after_two.delivery_target,
        after_alpha.delivery_state,
        after_alpha.delivery_target,
        after_beta.delivery_state,
        after_beta.delivery_target
    );
    let one_delivered = matches!(
        after_one.delivery_state,
        ChildDeliveryState::Delivered { .. }
    );
    // The sibling grandchild shares alpha-one's destination exactly, so its
    // staying Pending is what proves delivery is keyed by producer rather than
    // by target.
    let sibling_grandchild_untouched = after_two.delivery_state == ChildDeliveryState::Pending
        && after_two.delivery_target == Some(to_parent_child.clone());
    let children_untouched = after_alpha.delivery_state == ChildDeliveryState::Pending
        && after_alpha.delivery_target == Some(ChildDeliveryTarget::SessionOutbox)
        && after_beta.delivery_state == ChildDeliveryState::Pending
        && after_beta.delivery_target == Some(ChildDeliveryTarget::SessionOutbox);
    if one_delivered && sibling_grandchild_untouched && children_untouched {
        (
            Attribution::Correct,
            format!(
                "{detail} — the delivery landed only on the descendant that produced the result, \
                 including against a sibling bound for the identical target"
            ),
        )
    } else {
        (Attribution::Misattributed, detail)
    }
}

fn in_process_probe(case: &AttributionCase) -> (Attribution, String) {
    match case.event {
        LifecycleEvent::Reservation => probe_reservation(),
        LifecycleEvent::Refund => probe_refund_across_restart(),
        LifecycleEvent::Escalation => probe_escalation(),
        LifecycleEvent::Approval => probe_approval(),
        LifecycleEvent::Cancellation => probe_cancellation(),
        LifecycleEvent::Delivery => probe_delivery(),
    }
}

// ===========================================================================
// Running one case
// ===========================================================================

fn run_case(case: &AttributionCase) -> Vec<Execution> {
    let mut executions = Vec::new();
    let (attribution, detail) = in_process_probe(case);
    executions.push(Execution {
        event: case.event,
        mode: "in-process".to_owned(),
        attribution,
        detail: format!("{} :: {detail}", case.in_process_seam),
        live: None,
    });
    for transport in live::transports_for(case) {
        let outcome = live::live_probe(case, transport);
        executions.push(Execution {
            event: case.event,
            mode: outcome.transport.label().to_owned(),
            attribution: outcome.attribution,
            detail: outcome.evidence.observable.clone(),
            live: Some(outcome.evidence),
        });
    }
    executions
}

/// The outcome recorded for a case on this platform. MISATTRIBUTED dominates —
/// a misattribution observed on any surface is a misattribution — then
/// NOT-OBSERVABLE, then UNAVAILABLE, and only a wholly consistent set of correct
/// attributions reports CORRECT.
fn aggregate(executions: &[Execution]) -> Attribution {
    if executions
        .iter()
        .any(|e| e.attribution == Attribution::Misattributed)
    {
        return Attribution::Misattributed;
    }
    if executions
        .iter()
        .any(|e| e.attribution == Attribution::NotObservable)
    {
        return Attribution::NotObservable;
    }
    if executions
        .iter()
        .all(|e| e.attribution == Attribution::Unavailable)
    {
        return Attribution::Unavailable;
    }
    Attribution::Correct
}

fn ledger_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("child-attribution-corpus");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Emit the machine-readable rows for one case, to stdout and to the ledger, in
/// the exact shape `21-04-ATTRIBUTION-RESULTS.md` records — so the results
/// artifact is assembled from measurement rather than from recollection.
fn record(case: &AttributionCase, executions: &[Execution]) {
    let test_name = format!("attribution_{}", case.event.case_id());
    let mut rows = Vec::new();
    rows.push(format!(
        "CASE :: {test_name} :: {} :: {} :: {}",
        case.event.requirement_name(),
        platform(),
        aggregate(executions).label()
    ));
    rows.push(format!(
        "TOPOLOGY :: {test_name} :: siblings {} :: generations {} :: crash-and-restart {} :: \
         human-visible-surface {}",
        case.sibling_count,
        case.generations,
        case.crash_and_restart,
        case.human_visible_surface.label()
    ));
    for execution in executions {
        rows.push(format!(
            "MODE :: {test_name} :: {} :: {} :: {} :: {}",
            platform(),
            execution.mode,
            execution.attribution.label(),
            execution.detail
        ));
        if let Some(live) = &execution.live {
            rows.push(format!(
                "LIVE :: {test_name} :: {} :: {} :: {} :: {}",
                platform(),
                live.asserted_mode,
                live.invocation,
                live.transcript_path
            ));
        }
    }
    let body = rows.join("\n");
    println!("{body}");
    let _ = std::fs::write(
        ledger_dir().join(format!("{}.rows", case.event.case_id())),
        format!("{body}\n"),
    );
}

fn drive(event: LifecycleEvent) {
    let case = cases::case(event);
    let executions = run_case(case);
    record(case, &executions);

    assert_completeness(case, &executions);
    assert_live_runs_proved_their_mode(case, &executions);
    assert_mode_equivalence(case, &executions);
}

/// Recorded executions must equal one in-process run plus one per declared live
/// transport, each stamped with this case's own event. A row attributed to the
/// wrong event would land in the results table as evidence about something it
/// never exercised.
fn assert_completeness(case: &AttributionCase, executions: &[Execution]) {
    let expected = 1 + live::transports_for(case).len();
    assert_eq!(
        executions.len(),
        expected,
        "{}: recorded {} executions against {expected} declared (one in-process plus one per live \
         transport). A missing mode is a coverage hole, never an implicit pass.",
        case.event.requirement_name(),
        executions.len()
    );
    assert_eq!(
        executions.iter().filter(|e| e.mode == "in-process").count(),
        1,
        "{}: the in-process mode must be recorded exactly once",
        case.event.requirement_name()
    );
    for execution in executions {
        assert_eq!(
            execution.event,
            case.event,
            "a driver returned an execution stamped {} while running the {} case",
            execution.event.requirement_name(),
            case.event.requirement_name()
        );
        assert!(
            !execution.detail.trim().is_empty(),
            "{} ({}): recorded no observation, so the row is not evidence",
            case.event.requirement_name(),
            execution.mode
        );
    }
}

/// Every live execution must carry its four evidence fields, and any live run
/// that recorded a decisive verdict must have PROVED the mode it landed in.
fn assert_live_runs_proved_their_mode(case: &AttributionCase, executions: &[Execution]) {
    for execution in executions.iter().filter(|e| e.live.is_some()) {
        let live = execution.live.as_ref().expect("filtered on Some");
        assert!(
            live.invocation.contains("wayland-core"),
            "{} ({}): the recorded invocation does not name the real binary: {}",
            case.event.requirement_name(),
            execution.mode,
            live.invocation
        );
        assert!(
            !live.observable.trim().is_empty(),
            "{} ({}): no observable was recorded, so nothing distinguished correct attribution \
             from a misattribution",
            case.event.requirement_name(),
            execution.mode
        );
        if execution.attribution.is_decisive() {
            assert!(
                !live.asserted_mode.ends_with("-UNPROVEN"),
                "{} ({}): a verdict of {} was recorded from a run that never proved which mode it \
                 landed in ({})",
                case.event.requirement_name(),
                execution.mode,
                execution.attribution.label(),
                live.asserted_mode
            );
        }
    }
}

/// The assertion that catches the failure this codebase has already shipped
/// once: an in-process CORRECT standing against a live MISATTRIBUTED means the
/// plumbing is right and the product shows the user the wrong thing. It is
/// called out as its own class rather than folded into whatever red accompanies
/// it, because a wire-level test cannot see it and a human would report it as a
/// bug on day one.
fn assert_mode_equivalence(case: &AttributionCase, executions: &[Execution]) {
    let Some(in_process) = executions.iter().find(|e| e.mode == "in-process") else {
        return;
    };
    if in_process.attribution != Attribution::Correct {
        return;
    }
    for execution in executions.iter().filter(|e| e.live.is_some()) {
        assert_ne!(
            execution.attribution,
            Attribution::Misattributed,
            "MODE-EQUIVALENCE FAILURE (in-process vouched for attribution the shipped product \
             does not preserve) :: attribution_{} :: event {} :: surface {} :: in-process CORRECT \
             against live MISATTRIBUTED. Live detail: {}",
            case.event.case_id(),
            case.event.requirement_name(),
            execution.mode,
            execution.detail
        );
    }
}

// ===========================================================================
// Table-level invariants — no driver runs for these
// ===========================================================================

#[test]
fn corpus_table_covers_every_lifecycle_event() {
    for event in LIFECYCLE_EVENTS {
        let found = cases::CORPUS
            .iter()
            .filter(|entry| entry.event == *event)
            .count();
        assert_eq!(
            found,
            1,
            "F21-03 lifecycle event {} appears {found} times in the corpus table; it must appear \
             exactly once. F21-03 is the sole authorised source of cases, so an event cannot be \
             dropped and none may be invented.",
            event.requirement_name()
        );
    }
    assert_eq!(
        cases::CORPUS.len(),
        LIFECYCLE_EVENTS.len(),
        "the corpus table has {} entries against F21-03's {} lifecycle events",
        cases::CORPUS.len(),
        LIFECYCLE_EVENTS.len()
    );
}

/// THE SINGLE MOST IMPORTANT TABLE INVARIANT. One child under one parent
/// attributes correctly by accident, because there is nowhere else for anything
/// to go, so a single-child corpus goes green on a system that ignores
/// attribution entirely.
#[test]
fn every_case_runs_at_least_two_siblings() {
    for entry in cases::CORPUS {
        assert!(
            entry.sibling_count >= 2,
            "{}: the case declares {} sibling(s). Every case must run at least two, so a \
             misattribution has somewhere wrong to land and is detectable.",
            entry.event.requirement_name(),
            entry.sibling_count
        );
        assert!(
            entry.generations >= 2,
            "{}: the case declares {} generation(s); a case with no nesting is not testing a \
             NESTED lifecycle event.",
            entry.event.requirement_name(),
            entry.generations
        );
    }
    // The two events whose attribution question is about which ANCESTOR the
    // event rolls up to, rather than which peer owns it, must be nested deeper
    // than one level.
    for event in [LifecycleEvent::Escalation, LifecycleEvent::Delivery] {
        assert!(
            cases::case(event).generations >= 3,
            "{}: this event's attribution question is about which ancestor it rolls up to, so it \
             must run at least two levels deep",
            event.requirement_name()
        );
    }
}

#[test]
fn the_corpus_carries_a_crash_and_restart_case() {
    let restarts = cases::CORPUS
        .iter()
        .filter(|entry| entry.crash_and_restart)
        .count();
    assert!(
        restarts >= 1,
        "no case kills and restarts between the causing action and the event whose attribution is \
         asserted. Attribution that survives only inside one process lifetime is attribution \
         proved in the easy case only, and budget_authority.rs exists precisely because runtime \
         budget mutation is only useful if the same authority survives a crash."
    );
}

#[test]
fn every_invariant_states_an_attribution_and_names_no_error_shape() {
    // An invariant that names any of this is an assertion on today's failure
    // shape, which keeps passing for the wrong reason once the shape changes.
    const ERROR_SHAPE_WORDS: [&str; 8] = [
        "error",
        "err(",
        "panic",
        "status",
        "exit code",
        "message \"",
        "returns err",
        "result::",
    ];
    for entry in cases::CORPUS {
        assert!(
            !entry.request.trim().is_empty(),
            "{}: no nested request is described",
            entry.event.requirement_name()
        );
        let lowered = entry.invariant.to_ascii_lowercase();
        for word in ERROR_SHAPE_WORDS {
            assert!(
                !lowered.contains(word),
                "{}: the invariant names an error shape ({word:?}): {}",
                entry.event.requirement_name(),
                entry.invariant
            );
        }
        assert!(
            lowered.contains("must"),
            "{}: the invariant does not state an obligation: {}",
            entry.event.requirement_name(),
            entry.invariant
        );
        assert!(
            lowered.contains("sibling") || lowered.contains("parent") || lowered.contains("actor"),
            "{}: the invariant names no actor, so it is not an attribution invariant: {}",
            entry.event.requirement_name(),
            entry.invariant
        );
        assert!(
            !entry.live_observable.trim().is_empty(),
            "{}: no live observable is declared, so a live run could not distinguish correct \
             attribution from a misattribution",
            entry.event.requirement_name()
        );
    }
}

#[test]
fn the_rendered_screen_surface_declares_its_unavailability() {
    // The declared platform fact, stated rather than discovered at runtime.
    let tui_available = LiveTransport::Tui.available_here();
    assert_eq!(
        tui_available,
        !cfg!(windows),
        "the rendered-screen surface must be declared available exactly off Windows: {}",
        LiveTransport::Tui.unavailable_reason()
    );
    assert!(
        LiveTransport::JsonStream.available_here(),
        "the host-protocol surface must be available on every platform this corpus runs on"
    );
    // The two events a human answers or watches must be declared against the
    // rendered screen, not only the wire — otherwise the Windows limitation
    // this corpus is required to name would leave nothing unproved and would
    // silently disappear.
    for event in [LifecycleEvent::Approval, LifecycleEvent::Cancellation] {
        assert_eq!(
            cases::case(event).human_visible_surface,
            cases::HumanVisibleSurface::RenderedScreen,
            "{} is answered or watched by a human, so its proof must include the rendered screen",
            event.requirement_name()
        );
    }
    println!(
        "AVAILABILITY :: {} :: json-stream={} :: tui={tui_available}",
        platform(),
        LiveTransport::JsonStream.available_here()
    );
    println!(
        "LIMITATION :: windows-tui :: {} :: approval and cancellation as a human sees them",
        LiveTransport::Tui.unavailable_reason()
    );
}

/// The approval bridge's TTL reaper auto-resolves an expired entry as
/// cancelled. That is an attribution question in its own right: a reaper that
/// swept both siblings when only one expired would cancel work nobody asked to
/// cancel.
#[test]
fn the_ttl_reaper_cancels_only_the_expired_sibling() {
    let rt = runtime();
    rt.block_on(async {
        let bridge = ApprovalBridge::with_ttl(Duration::from_millis(40));
        let long = ApprovalBridge::with_ttl(Duration::from_secs(3_600));
        let request = |call_id: &str| ApprovalRequest {
            call_id: call_id.to_owned(),
            reason: "the corpus asks the sibling to mutate".to_owned(),
            context: "child-attribution corpus".to_owned(),
        };
        let (_short_token, mut short_rx) = bridge
            .request_with_id(SIBLING_A.to_owned(), request(SIBLING_A))
            .await;
        let (_long_token, mut long_rx) = long
            .request_with_id(SIBLING_B.to_owned(), request(SIBLING_B))
            .await;
        tokio::time::sleep(Duration::from_millis(120)).await;
        let reaped = bridge.reap_now().await;
        let short_answer = short_rx.try_recv();
        let long_answer = long_rx.try_recv();
        println!(
            "REAPER :: reaped {reaped} :: expired sibling outcome {short_answer:?} :: \
             unexpired sibling outcome {long_answer:?}"
        );
        assert_eq!(
            reaped, 1,
            "the reaper swept {reaped} entries when exactly one sibling's approval had expired"
        );
        assert!(
            short_answer.is_ok(),
            "the expired sibling's approval was not auto-resolved"
        );
        assert!(
            long_answer.is_err(),
            "the unexpired sibling's approval was resolved by another sibling's expiry, which \
             cancels work nobody asked to cancel"
        );
    });
}

// ===========================================================================
// The six corpus cases. One test per F21-03 lifecycle event, so every case id
// appears in the run transcript and a case that did not execute cannot be
// mistaken for one that passed.
// ===========================================================================

#[test]
fn attribution_reservation() {
    drive(LifecycleEvent::Reservation);
}

#[test]
fn attribution_refund() {
    drive(LifecycleEvent::Refund);
}

#[test]
fn attribution_escalation() {
    drive(LifecycleEvent::Escalation);
}

#[test]
fn attribution_approval() {
    drive(LifecycleEvent::Approval);
}

#[test]
fn attribution_cancellation() {
    drive(LifecycleEvent::Cancellation);
}

#[test]
fn attribution_delivery() {
    drive(LifecycleEvent::Delivery);
}
