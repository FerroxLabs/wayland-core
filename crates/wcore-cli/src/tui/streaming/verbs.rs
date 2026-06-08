//! The streaming verb pool + single-pick mechanics (v0.9.2 W6, SPEC §4).
//!
//! The streaming-status widget shows a playful gerund verb while a turn is
//! in flight ("✻ Stoking the forge… (12s · ↑ 88 tokens · drafting reply)").
//! Unlike v0.9.1's time-based rotation, the verb is **sampled once per turn**
//! and held for the whole turn — the `useState(|| sample(pool))` pattern.
//! [`pick_turn_verb`] is the pure sampler; the per-turn seed lives on
//! `SessionView::turn_verb_seed` (set once at `StreamStart` by the protocol
//! bridge), so every frame in a turn renders the same verb and consecutive
//! turns land on different verbs.
//!
//! ## Pool provenance (SPEC §4 sourcing recipe)
//!
//! The pool is Wayland's own voice — gerund-verbs (Claude Code's *grammar*)
//! with Forge's *playfulness* and Hearth/forge brand flavor. It is built from
//! three sources, none copied verbatim:
//!
//! 1. **Forge-tone rewrites** — Forge's 130 witty phrases
//!    (`wittyPhrases.ts`, Apache-2.0 / Google-LLC, *tone reference only*) are
//!    mostly meme/pop-culture lines, not gerunds. Their *playfulness* is mined
//!    and re-expressed as Wayland gerunds (e.g. Forge's "Brewing fresh bytes"
//!    → our "Brewing fresh bytes" is paraphrased to "Steeping the bytes").
//! 2. **CC rewrites** — Claude Code's ~190 gerund-verbs are fully reworded so
//!    NONE are direct copies. A blocklist test (`pool_has_zero_cc_verbs`)
//!    asserts the ~20 most distinctive CC verbs never ship.
//! 3. **Wayland/Hearth/forge originals** — brand-flavored gerunds like
//!    "Stoking the forge", "Tempering the steel", "Banking the coals".
//!
//! The pool is `&'static [&'static str]` — zero allocation, embedded in the
//! binary. Acceptance: ≥150 entries, zero duplicates, zero CC-verbatim copies.

/// The streaming verb pool. Each entry is a gerund phrase in Wayland's voice:
/// forge/hearth craft imagery, light playfulness, and earnest "working on it"
/// phrasing. ≥150 entries, all unique, none copied verbatim from Claude Code
/// or Forge (see module docs for provenance).
///
/// Display appends an ellipsis ("…") at the render site, so entries here carry
/// NO trailing punctuation.
pub static SPINNER_VERBS: &[&str] = &[
    // ── Forge / hearth / smithing originals (Wayland brand voice) ──
    "Stoking the forge",
    "Tending the hearth",
    "Tempering the steel",
    "Quenching the blade",
    "Folding the steel",
    "Banking the coals",
    "Hammering the anvil",
    "Drawing out the billet",
    "Working the bellows",
    "Raking the embers",
    "Setting the rivets",
    "Truing the edge",
    "Filing the burr",
    "Striking while hot",
    "Coaxing the flame",
    "Feeding the firebox",
    "Forging the chain",
    "Annealing the work",
    "Casting the ingot",
    "Pouring the crucible",
    "Sharpening the bit",
    "Oiling the hinges",
    "Setting the keystone",
    "Squaring the corners",
    "Planing the grain",
    "Joining the dovetails",
    "Sanding the edges",
    "Mortising the joint",
    "Clamping the glue-up",
    "Whetting the chisel",
    // ── Thinking / reasoning gerunds ──
    "Mulling it over",
    "Turning it over",
    "Chewing on it",
    "Weighing the options",
    "Connecting the dots",
    "Following the thread",
    "Tracing the logic",
    "Untangling the knot",
    "Squinting at the problem",
    "Sketching the approach",
    "Mapping the terrain",
    "Charting a course",
    "Plotting the route",
    "Sizing it up",
    "Reading between the lines",
    "Joining the threads",
    "Reasoning it through",
    "Sleeping on it briefly",
    "Letting it marinate",
    "Considering the angles",
    "Pondering the shape of it",
    "Lining up the dominoes",
    "Threading the needle",
    "Doing the mental math",
    "Running the numbers",
    "Picturing the outcome",
    "Imagining the next step",
    "Reconciling the constraints",
    "Holding the whole thing in view",
    "Looking before leaping",
    // ── Kitchen / brewing / craft imagery (Forge-tone rewrites) ──
    "Brewing the response",
    "Steeping the bytes",
    "Leavening the dough",
    "Proofing the loaf",
    "Kneading the logic",
    "Whisking it together",
    "Simmering the broth",
    "Reducing the sauce",
    "Folding in the details",
    "Plating the answer",
    "Garnishing the reply",
    "Setting the table",
    "Letting it rest",
    "Tasting for seasoning",
    "Stirring the pot",
    "Toasting the edges",
    "Caramelizing the surface",
    "Pressing the cider",
    "Bottling the result",
    "Decanting the answer",
    // ── Building / assembling / engineering gerunds ──
    "Assembling the pieces",
    "Wiring it up",
    "Bolting it together",
    "Soldering the joints",
    "Fitting the parts",
    "Torquing the bolts",
    "Calibrating the gauges",
    "Aligning the gears",
    "Greasing the cogs",
    "Spinning up the works",
    "Priming the pump",
    "Topping off the tank",
    "Warming the engine",
    "Tuning the carburetor",
    "Balancing the load",
    "Stacking the blocks",
    "Squaring the frame",
    "Laying the foundation",
    "Pouring the slab",
    "Raising the rafters",
    "Stringing the cable",
    "Routing the wires",
    "Patching it through",
    "Closing the loop",
    "Sealing the seams",
    "Buttoning it up",
    "Tightening the screws",
    "Lining up the parts",
    "Snapping it into place",
    "Dialing it in",
    // ── Search / gathering / surveying gerunds ──
    "Surveying the scene",
    "Scouting ahead",
    "Combing the records",
    "Sifting the findings",
    "Panning for gold",
    "Casting the net",
    "Gathering the threads",
    "Rummaging the shelves",
    "Foraging for clues",
    "Digging into it",
    "Spelunking the codebase",
    "Excavating the details",
    "Dredging the archives",
    "Canvassing the options",
    "Triangulating the answer",
    "Sounding the depths",
    "Reading the map",
    "Walking the perimeter",
    "Checking the corners",
    "Taking inventory",
    // ── Writing / drafting / shaping gerunds ──
    "Drafting the reply",
    "Penning the response",
    "Sharpening the prose",
    "Trimming the fat",
    "Polishing the phrasing",
    "Finding the right words",
    "Choosing the verbs",
    "Tightening the draft",
    "Outlining the answer",
    "Threading the argument",
    "Shaping the response",
    "Wordsmithing it",
    "Editing on the fly",
    "Inking the page",
    "Composing the lines",
    "Rounding out the edges",
    "Filling in the blanks",
    "Connecting the paragraphs",
    "Stitching it together",
    "Drawing the conclusions",
    // ── Light / playful / whimsical gerunds (Forge-tone, rewritten) ──
    "Consulting the rubber duck",
    "Asking the oracle",
    "Spinning the dials",
    "Counting the electrons",
    "Herding the photons",
    "Chasing the runtime",
    "Coaxing the daemon",
    "Bribing the cache",
    "Negotiating with the linker",
    "Wrangling the pointers",
    "Petting the type system",
    "Befriending the borrow checker",
    "Summoning the answer",
    "Channeling the muse",
    "Reticulating the splines",
    "Buffering brilliance",
    "Defragmenting thoughts",
    "Recompiling the plan",
    "Warming up the cores",
    "Spooling up",
    "Catching the train of thought",
    "Aligning the stars",
    "Waiting on the muse",
    "Loading the good part",
    "Chasing down the answer",
    "Cranking the handle",
    "Turning the crank",
    "Greasing the wheels",
    "Winding the spring",
    "Setting the gears in motion",
];

/// Sample the verb for a turn from a per-turn seed.
///
/// Pure and deterministic: the same `seed` always yields the same verb, so
/// every render frame in a turn shows the identical verb (no per-frame
/// flicker). This is the single-pick equivalent of `useState(|| sample())` —
/// the seed is generated once at `StreamStart` and stored on
/// `SessionView::turn_verb_seed`, NOT a time-based rotation.
pub fn pick_turn_verb(seed: u64) -> &'static str {
    SPINNER_VERBS[(seed as usize) % SPINNER_VERBS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ~20 of the most distinctive Claude Code spinner verbs. These are the
    /// copyrighted strings the pool must NEVER ship — the blocklist test
    /// (`pool_has_zero_cc_verbs`) is the automated guard SPEC §4 requires.
    const CC_BLOCKLIST: &[&str] = &[
        "Boondoggling",
        "Discombobulating",
        "Razzle-dazzling",
        "Flibbertigibbeting",
        "Whatchamacalliting",
        "Bamboozling",
        "Cattywampusing",
        "Frabjousing",
        "Gallivanting",
        "Skedaddling",
        "Whirligigging",
        "Snollygostering",
        "Mumbo-jumboing",
        "Lollygagging",
        "Brouhahaing",
        "Collywobbling",
        "Wibbling",
        "Flummoxing",
        "Hornswoggling",
        "Codswalloping",
    ];

    #[test]
    fn pool_has_at_least_150_verbs() {
        assert!(
            SPINNER_VERBS.len() >= 150,
            "verb pool is only {} entries; SPEC §4 requires ≥150",
            SPINNER_VERBS.len()
        );
    }

    #[test]
    fn pool_has_zero_cc_verbs() {
        // SPEC §4 acceptance: zero entries match Claude Code's known verbs.
        // Exact-match guard against the embedded blocklist (these are the
        // distinctive CC verbs — copyright). Compared case-insensitively so a
        // capitalization tweak doesn't sneak a copy through.
        for v in SPINNER_VERBS {
            let lower = v.to_ascii_lowercase();
            for cc in CC_BLOCKLIST {
                assert_ne!(
                    lower,
                    cc.to_ascii_lowercase(),
                    "verbatim CC verb leaked into the pool: {v:?}"
                );
            }
        }
    }

    #[test]
    fn pool_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for v in SPINNER_VERBS {
            assert!(seen.insert(*v), "duplicate verb in the pool: {v:?}");
        }
    }

    #[test]
    fn pool_entries_carry_no_trailing_ellipsis_or_whitespace() {
        // The render site appends "… " — entries must not double it up or
        // carry stray whitespace that would misalign the status line.
        for v in SPINNER_VERBS {
            assert!(
                !v.ends_with('…'),
                "entry carries a trailing ellipsis: {v:?}"
            );
            assert!(!v.ends_with("..."), "entry carries trailing dots: {v:?}");
            assert_eq!(*v, v.trim(), "entry carries stray whitespace: {v:?}");
            assert!(!v.is_empty(), "empty entry in the pool");
        }
    }

    #[test]
    fn pick_is_constant_for_a_seed() {
        // Same seed → same verb, every call. This is the per-turn stability
        // guarantee: a turn's seed is fixed, so the verb never flickers.
        assert_eq!(pick_turn_verb(7), pick_turn_verb(7));
        assert_eq!(pick_turn_verb(0), pick_turn_verb(0));
        assert_eq!(pick_turn_verb(u64::MAX), pick_turn_verb(u64::MAX));
    }

    #[test]
    fn pick_varies_across_seeds() {
        // Probabilistic: across a span of seeds the chosen verb is not always
        // the same one — consecutive turns land on different verbs.
        let distinct: std::collections::HashSet<_> = (0..50u64).map(pick_turn_verb).collect();
        assert!(
            distinct.len() > 1,
            "pick_turn_verb returned only one distinct verb across 50 seeds"
        );
    }

    #[test]
    fn pick_indexes_within_bounds_for_extreme_seeds() {
        // No panic on the modulo for boundary seeds.
        let _ = pick_turn_verb(0);
        let _ = pick_turn_verb(u64::MAX);
        let _ = pick_turn_verb(SPINNER_VERBS.len() as u64);
        let _ = pick_turn_verb(SPINNER_VERBS.len() as u64 - 1);
    }
}
