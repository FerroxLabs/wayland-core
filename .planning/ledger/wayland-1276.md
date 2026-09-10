---
issue: 1276
repo: FerroxLabs/wayland
kind: defect
title: "No standing gate stops a FOURTH hand-cut URL authority parser (split from #1252 c3)"
status: open
last_verified_commit: 7033a0d3f
criteria:
  - id: c1
    text: "Adding a function to `crates/` that returns a host- or authority-shaped value by string surgery rather than through `url::Url` / `wcore_types::url_authority` fails a gate rather than passing silently -- shown RED by adding one."
    state: met
    evidence: "file:scripts/check-url-authority-cuts.py:507:UNDISPOSITIONED %s:%d: `fn"
    owner: core
    note: "MET at 7033a0d3f. `scripts/check-url-authority-cuts.py` fails on a host- or authority-shaped value produced by string surgery, and it was SHOWN RED by adding one, twice, against the real tree rather than against a fixture. ARM 1, a fourth cut carrying the scheme literal, appended to crates/wcore-providers/src/anthropic.rs: `pub fn provider_host(raw: &str) -> Option<String>` cutting on `split_once(\"://\")` then `split('/')` then `rsplit('@')` -- RC=1, two findings, one from each axis. ARM 2, the same defect with NO scheme literal anywhere: `pub fn upstream_authority(raw: &str) -> &str` built out of `find(\"//\")` and a slice -- RC=1, one finding, from axis B alone, which is the axis that exists for exactly this. Both removed with `git checkout --` and the tree re-verified RC=0 after each. The baseline before and after both arms is RC=0, so the reds are attributable to the cut and not to a gate that fails on everything. WHAT WOULD FALSIFY THIS: deleting the axis-B branch this line anchors, which is the only thing that sees a cut carrying no scheme literal. RESIDUAL, stated rather than papered over: a cut whose function is named for neither a host nor an authority AND which carries no `\"://\"` literal -- `fn cut(raw: &str) -> String` -- is seen by neither axis. Enumerating every fn returning a String instead would convict thousands of innocent functions, which is how a gate gets switched off; under-attribution is the deliberate direction here and is documented in the script's own header."
  - id: c2
    text: "The gate`s enumeration is a total syntactic set over production sources, not a list of cutting idioms, and it carries an anti-vacuity control that fails CLOSED when its own enumeration stops matching (the `sites >= N` shape `wayland-core#402` established)."
    state: met
    evidence: "file:scripts/check-url-authority-cuts.py:113:MIN_SCHEME_SITES = 18"
    owner: core
    note: "MET at 7033a0d3f. The enumeration is TOTAL over two declared syntactic classes, neither of which is a list of idioms. Axis A is every line under `crates/*/src` carrying the literal `\"://\"` -- the same set #1252's own closing measurement used, and it reproduces that measurement's count exactly: 24 at `488fbbae9`, 24 here. It is a superset of every cutting idiom rather than an enumeration of them, and it catches `find(\"://\")`, the spelling that escaped #1252's first sweep and produced Site C. Axis B is every `fn` outside `#[cfg(test)]` whose name carries a host/authority word AND whose return type, unwrapped to the VALUE position, is string- or Host-shaped: 28 declarations. Reading the ERROR position instead swept in 42 innocent `host_register_*` plugin trait methods whose `host` is the host APPLICATION -- that draft was measured and discarded, and the value-position rule is documented at `host_shaped_return`. ANTI-VACUITY IS PER AXIS AND FAILS CLOSED, the `sites >= N` shape `wayland-core#402` established: floors of 18 and 22 against measured 24 and 28. SHOWN RED: replacing the scheme literal with `\"//:\"` and the host-word pattern with `zzzz` -- the two ways an enumeration silently stops matching -- gives RC=1 with `axis A found 0 ... floor is 18` and `axis B found 0 ... floor is 22`, not a clean sweep. The gate's own `--self-test` carries the same arm over an empty synthetic tree. WHAT WOULD FALSIFY THIS: deleting either floor, which turns a quiet enumeration back into a pass."
  - id: c3
    text: "Every site the enumeration finds is classified -- `answers through the parser`, `returns no host`, or `renders without deciding` -- with the reason recorded where the gate reads it, so the four already-dispositioned display cuts and `events.rs::split_endpoint` stay green without weakening it."
    state: met
    evidence: "file:.config/url-authority-dispositions.txt:77:display    crates/wcore-protocol/src/events.rs::split_endpoint"
    owner: core
    note: "MET at 7033a0d3f. Every one of the 52 enumerated sites is classified where the gate READS it -- `.config/url-authority-dispositions.txt`, 44 hand-written lines plus 8 that need none because the gate reads the parser call in the body itself. Classes are c3's own three (`parser`, `no-host`, `display`) plus `test-only`, which the enumeration forced: #1252's sweep excluded `crates/*/tests/` but four scheme-separator sites live in `#[cfg(test)]` modules inside `src/`, and calling those `returns no host` would be false -- they do cut a host out of the bundled `providers.toml`. Naming what they are beats mislabelling them, and the addition is stated in the file's own header. THE FIVE SITES THIS CRITERION NAMES ARE ALL GREEN WITHOUT WEAKENING ANYTHING: `sources_block.rs::extract_domain`, `tool_formatters/web.rs::derive_domain`, `tool_formatters/web_fetch.rs::derive_domain` are `display`, each with the reason it decides nothing; `events.rs::split_endpoint` is `display` carrying #1243's own stated remainder -- retained only as `scrub_base_url`'s redaction fallback for a string that does not parse as a URL at all, off the `is_local_endpoint` path. A line is keyed on path, symbol AND a fingerprint of the site's own text, so a SECOND cut added to a classified function is a new site that reds the gate with the line still sitting here, and EDITING a classified site changes its fingerprint and goes STALE rather than inheriting the old disposition. A line matching nothing fails as stale; a line whose reason is under 25 characters is refused as unparseable rather than read as an exemption -- both proven by `--self-test`. WHAT WOULD FALSIFY THIS: dropping the fingerprint from the key, which would let a cut be edited into a decision under cover of an old disposition."
  - id: c4
    text: "The three sites `#1252` fixed and the two `#1243` fixed are all seen by the enumeration -- the known-positive control, so a gate that finds nothing cannot pass."
    state: met
    evidence: "file:scripts/check-url-authority-cuts.py:102:KNOWN_POSITIVES = ("
    owner: core
    note: "MET at 7033a0d3f. All five are seen, and they are NAMED rather than derived for a reason the criterion's own framing hides: the FIXES removed their scheme literals, so a control derived from axis A or B would have deleted itself along with the defect. `doctor/mod.rs::base_url_caveat` and `browser/policy.rs::strip_pattern_decorations` carry no `\"://\"` and no host-shaped name; a derived control would report four of five and call it a full sweep. So the five are declared, each must EXIST and must still reach `url::Url` / `wcore_types::url_authority`, and either failure fails the gate whatever else it found. Run output names all five: `known-positive ok:` for wayland#1252 Sites A/B/C and wayland#1243 Sites A/B. SHOWN RED: reverting `webfetch.rs::host_of` (#1243 Site A) to the pre-fix hand cut gives RC=1 with `KNOWN-POSITIVE REGRESSED: ... no longer reaches url::Url / wcore_types::url_authority. This is the bug the originating issue fixed, arriving again at its own site`, plus two undispositioned-site findings from the axes. Restored with `git checkout --`, tree re-verified RC=0. The self-test carries the complementary arm: over a tree where the five files do not exist, `KNOWN-POSITIVE GONE` fires, so a gate that finds nothing cannot pass. WHAT WOULD FALSIFY THIS: shortening KNOWN_POSITIVES, which is why the tuple itself is the anchor."
---

Created 2026-08-31 to close a COVERAGE gap. It records no work as done.

`scripts/check-release-readiness.py` reads ledger files and nothing else, so an open in-scope issue with no ledger is invisible to it. `check-criteria-ledger.py`'s
coverage arm is the only thing that reports the gap, and CI runs that arm
`--offline`, which cannot ask the trackers -- so nothing said so.

Criteria are transcribed from the issue body WITHOUT EDIT. Where the wording is
loose it is left loose: sharpening a criterion inside the ledger is how a
criterion quietly becomes an easier adjacent property. Whoever takes this
restates it on the ISSUE first.


Provenance carried over from the issue, because it changes how c1 may be graded:
the issue attaches NO red arm and says so -- the defect is a counterfactual about a
cut that does not exist yet, and the filing lane declined to manufacture one rather
than report a modelled failure as a measured one. c1's own text supplies the arm
(`shown RED by adding one`), so whoever takes this adds a cut, watches the gate red,
and removes it. The 24-hit sweep in the body IS measured, at `lane/f13-authority`
@ `488fbbae9`, and found no undispositioned production site -- so this is a
STANDING-GATE gap, not a live parsing defect.


## 2026-09-10 -- the gate

Built on `w14/gate`. `scripts/check-url-authority-cuts.py`, wired into
`scripts/preflight.sh` and `.github/workflows/ci.yml`, with dispositions in
`.config/url-authority-dispositions.txt`.

The counterfactual the filing lane declined to manufacture is now manufactured
and measured, twice, against the real tree: a fourth cut carrying the scheme
literal and a fourth cut carrying none. Both red, both removed, baseline green
either side.

WHAT THIS GATE STILL CANNOT SEE, stated here because a reader of a met row
deserves it: a hand cut whose function is named for neither a host nor an
authority and which carries no scheme literal is invisible to both axes. That
is under-attribution, chosen deliberately -- the alternative enumerates every
`String`-returning function in the workspace and gets switched off in a week --
and it is the residual anyone filing the FIFTH instance of this class should
read first.
