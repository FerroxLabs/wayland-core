//! The child-ATTRIBUTION corpus, expressed as DATA — Phase 21, plan 21-04.
//!
//! ## Why this table exists beside `child_authority_corpus/cases.rs`
//!
//! That corpus asks Success Criterion 1's question: *did the child obtain MORE
//! than the parent held?* This one asks Success Criterion 2's, which is a
//! different property and needs its own proof: *whose books did the child's
//! activity land on?* A system can be perfectly non-amplifying and still
//! misattribute — the child gets no extra authority, but its refund credits the
//! wrong parent, its approval prompt is answered by a different session, or its
//! result is delivered to a sibling. Nothing in the authority corpus tests that,
//! because every one of its verdicts is about what the child OBTAINED.
//!
//! ## The single most important construction rule in this file
//!
//! EVERY case runs with AT LEAST TWO SIBLINGS under a parent, and where nesting
//! is the point, at least two levels deep. One child under one parent attributes
//! correctly by accident — there is nowhere else for anything to go — so a
//! single-child corpus goes green on a system that ignores attribution entirely.
//! Two siblings give a misattribution somewhere wrong to land, which is the only
//! thing that makes it detectable. `sibling_count` is asserted `>= 2` by a
//! table-level invariant in the harness; it is not a convention.
//!
//! ## What an invariant may and may not say
//!
//! Same two prohibitions the authority corpus carries, for the same reasons:
//!
//! 1. **No error shape.** No entry names an error string, error kind, error
//!    variant or numeric status. An assertion on today's failure shape keeps
//!    passing for the wrong reason the moment the shape changes.
//! 2. **No mechanism.** The invariant says which actor the event must be
//!    attributable to, never which bookkeeping structure is expected to carry
//!    the attribution.
//!
//! Every invariant here is phrased as *the event must remain attributable to the
//! actor that caused it, and must not land on a sibling*.

/// The six lifecycle events F21-03 names. This list is the SOLE authorised
/// source of cases: an event F21-03 did not name gets no entry, and none of the
/// six may be dropped. `corpus_table_covers_every_lifecycle_event` binds the
/// table to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LifecycleEvent {
    Reservation,
    Refund,
    Escalation,
    Approval,
    Cancellation,
    /// F21-03 spells this "result delivery"; the case id is `delivery`.
    Delivery,
}

pub const LIFECYCLE_EVENTS: &[LifecycleEvent] = &[
    LifecycleEvent::Reservation,
    LifecycleEvent::Refund,
    LifecycleEvent::Escalation,
    LifecycleEvent::Approval,
    LifecycleEvent::Cancellation,
    LifecycleEvent::Delivery,
];

impl LifecycleEvent {
    /// The case identifier, which is also the suffix of the corpus test name and
    /// the token the results table indexes rows by.
    pub const fn case_id(self) -> &'static str {
        match self {
            Self::Reservation => "reservation",
            Self::Refund => "refund",
            Self::Escalation => "escalation",
            Self::Approval => "approval",
            Self::Cancellation => "cancellation",
            Self::Delivery => "delivery",
        }
    }

    /// The event name exactly as F21-03 spells it.
    pub const fn requirement_name(self) -> &'static str {
        match self {
            Self::Delivery => "result delivery",
            other => other.case_id(),
        }
    }
}

/// Which shipped live surface this event's attribution is READ from.
///
/// The distinction is not cosmetic. Approval and cancellation are the two
/// events where attribution is a thing a HUMAN does rather than a field a
/// machine reads: a person sees a prompt and answers it, and a person cancels
/// work and sees what stopped. A wire-level assertion does not prove the person
/// saw the right thing, so those two are read off the rendered TUI screen as
/// well as off the wire. The TUI is unavailable on Windows and that is DECLARED,
/// never silently skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanVisibleSurface {
    /// The host-protocol wire alone carries this event's attribution.
    Wire,
    /// A human answers or observes this event on a rendered screen, so the
    /// rendered screen is part of the proof.
    RenderedScreen,
}

impl HumanVisibleSurface {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Wire => "wire",
            Self::RenderedScreen => "rendered-screen",
        }
    }
}

/// One corpus entry: a nested lifecycle event, the sibling topology it is
/// exercised under, and the attribution invariant that must survive it.
#[derive(Debug, Clone, Copy)]
pub struct AttributionCase {
    pub event: LifecycleEvent,
    /// How many SIBLING children run under the parent for this case. Never
    /// fewer than two — see the module header.
    pub sibling_count: usize,
    /// How many generations deep the case runs. Two means parent → child;
    /// three means parent → child → grandchild, which is required wherever the
    /// attribution question is about which ANCESTOR the event rolls up to
    /// rather than merely which peer it belongs to.
    pub generations: usize,
    /// What the nested actor does.
    pub request: &'static str,
    /// Which actor the event must remain attributable to. Never an error shape,
    /// never a mechanism.
    pub invariant: &'static str,
    /// Whether this case kills and restarts between the causing action and the
    /// event whose attribution is asserted. At least one case must, because
    /// attribution that survives only inside one process lifetime proves
    /// attribution in the easy case only.
    pub crash_and_restart: bool,
    /// The real in-process seam the event is driven through. Named so a reader
    /// can check the corpus reached the product rather than a stand-in.
    pub in_process_seam: &'static str,
    pub human_visible_surface: HumanVisibleSurface,
    /// What a live run must show to distinguish CORRECT attribution from a
    /// MISATTRIBUTION. If the surface does not produce it, the case records
    /// NOT-OBSERVABLE with what was and was not seen — it is never asserted
    /// weakly and no production observability hook is added to make it
    /// possible.
    pub live_observable: &'static str,
}

/// The corpus. Six entries, one per F21-03 lifecycle event, in F21-03 order.
pub const CORPUS: &[AttributionCase] = &[
    AttributionCase {
        event: LifecycleEvent::Reservation,
        // Two siblings reserving against one parent envelope. With one child
        // the reserved total and the child's own reservation are the same
        // number, so a tracker that ignored the requester entirely would look
        // correct.
        sibling_count: 2,
        generations: 2,
        request: "Two sibling children each reserve provider headroom against the same parent \
                  envelope, with different amounts, so the two reservations are distinguishable \
                  by size as well as by owner.",
        invariant: "each sibling's reservation must remain attributable to the sibling that made \
                    it, and must not appear on the other sibling's books",
        crash_and_restart: false,
        in_process_seam: "wcore_budget::tracker::BudgetTracker::reserve + reserved_totals",
        human_visible_surface: HumanVisibleSurface::Wire,
        live_observable: "a per-sibling reservation observation on the host-protocol wire, keyed \
                          to the sibling that made it",
    },
    AttributionCase {
        event: LifecycleEvent::Refund,
        sibling_count: 2,
        generations: 2,
        // THE CRASH-AND-RESTART CASE. `budget_authority.rs` exists precisely
        // because runtime budget mutation is only useful if the same authority
        // survives a crash, and it appends the resulting authority before
        // reporting a mutation committed. A refund proved only inside one
        // process lifetime proves the easy half.
        request: "Two sibling children reserve against the same parent envelope; the process is \
                  killed and the authority rebound from the same journal; only one sibling's \
                  reservation is then refunded.",
        invariant: "the refund must remain attributable to the sibling whose reservation it \
                    releases, across a restart, and must not credit the other sibling",
        crash_and_restart: true,
        in_process_seam: "BudgetAuthorityCoordinator::bind over a SessionJournal, dropped, the \
                          durable authority reloaded from the journal file, rebound, then \
                          BudgetTracker::session_totals + reserved_totals + release",
        human_visible_surface: HumanVisibleSurface::Wire,
        live_observable: "a per-sibling refund observation on the host-protocol wire, keyed to \
                          the sibling whose reservation was released",
    },
    AttributionCase {
        event: LifecycleEvent::Escalation,
        sibling_count: 2,
        // Three generations: an escalation's whole question is which ANCESTOR
        // the extra headroom lands on, so a grandchild is required to make a
        // roll-up to the wrong ancestor detectable.
        generations: 3,
        request: "A grandchild exhausts its envelope and an operator-authorised extension is \
                  granted to it; a sibling grandchild under the same parent has asked for \
                  nothing.",
        invariant: "the escalation must remain attributable to the descendant that exhausted its \
                    envelope, and must not widen the sibling or any actor that did not ask",
        crash_and_restart: false,
        in_process_seam: "wcore_budget::tracker::BudgetTracker::extend_session + \
                          effective_session_limits",
        human_visible_surface: HumanVisibleSurface::Wire,
        live_observable: "a per-sibling escalation observation on the host-protocol wire, keyed \
                          to the descendant that exhausted its envelope",
    },
    AttributionCase {
        event: LifecycleEvent::Approval,
        sibling_count: 2,
        generations: 2,
        // A human answers this one. The wire alone cannot prove the human saw
        // the right thing, which is why the rendered screen is part of the
        // proof on every platform whose PTY driver exists.
        request: "Two sibling children each raise an approval under the same parent, and exactly \
                  one of the two is answered.",
        invariant: "the answer must resolve only the sibling whose approval it was given for, \
                    and the unanswered sibling must remain outstanding",
        crash_and_restart: false,
        in_process_seam: "wcore_agent::approval::ApprovalBridge::request_with_id + \
                          resolve_by_correlation",
        human_visible_surface: HumanVisibleSurface::RenderedScreen,
        live_observable: "which sibling the approval prompt names, on the wire and on the \
                          rendered screen a human answers",
    },
    AttributionCase {
        event: LifecycleEvent::Cancellation,
        sibling_count: 2,
        generations: 2,
        // A human sees this one too: they cancel work and watch what stops.
        request: "Two sibling children run under the same parent and exactly one of the two is \
                  cancelled.",
        invariant: "the cancellation must stop only the sibling it was directed at, and the \
                    other sibling must remain running",
        crash_and_restart: false,
        in_process_seam: "wcore_agent::durable_child::DurableChildStore::transition \
                          (RequestCancel) + inspect",
        human_visible_surface: HumanVisibleSurface::RenderedScreen,
        live_observable: "which sibling stops, on the wire and on the rendered screen a human \
                          watches",
    },
    AttributionCase {
        event: LifecycleEvent::Delivery,
        sibling_count: 2,
        // Three generations: `ChildDeliveryTarget::ParentChild { child_id }`
        // only becomes distinguishable from `ParentTurn` when a child has a
        // child of its own, so two levels would leave the variant untested.
        generations: 3,
        request: "Two sibling children finish with different results and different delivery \
                  targets — one to the parent turn, one to a nested parent child — and both are \
                  delivered.",
        invariant: "each result must remain attributable to the sibling that produced it and be \
                    delivered to that sibling's own target, and must not reach the other \
                    sibling's target",
        crash_and_restart: false,
        in_process_seam: "wcore_agent::durable_child::DurableChildStore::transition \
                          (DeliveryStarted / DeliveryDelivered) over ChildDeliveryTarget",
        human_visible_surface: HumanVisibleSurface::Wire,
        live_observable: "each sibling's own result text arriving under that sibling's \
                          parent_call_id on the host-protocol wire, and under no other",
    },
];

/// Look up the entry for a lifecycle event. Panics rather than returning an
/// option: a missing event is a corpus defect, and the completeness assertion in
/// the harness exists to catch it before any driver runs.
pub fn case(event: LifecycleEvent) -> &'static AttributionCase {
    CORPUS
        .iter()
        .find(|entry| entry.event == event)
        .unwrap_or_else(|| {
            panic!(
                "the attribution corpus has no entry for F21-03 lifecycle event {}",
                event.requirement_name()
            )
        })
}
