---
issue: 1276
repo: FerroxLabs/wayland
kind: defect
title: "No standing gate catches a fourth hand-cut authority parser: #1252 c3 closed the three known sites by measurement, not by inversion"
status: open
last_verified_commit: 488fbbae9
criteria:
  - id: c1
    text: "Adding a function to `crates/` that returns a host- or authority-shaped value by string surgery rather than through `url::Url` / `wcore_types::url_authority` fails a gate rather than passing silently -- shown RED by adding one."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT STARTED. Filed 2026-08-31 by lane f13-authority while closing wayland#1252, whose c3 is met AS WRITTEN (a property of the two named sites) but delivers no standing gate. Nothing in the workspace fails when a FOURTH hand cut is added."
  - id: c2
    text: "The gate's enumeration is a total syntactic set over production sources, not a list of cutting idioms, and it carries an anti-vacuity control that fails CLOSED when its own enumeration stops matching (the `sites >= N` shape wayland-core#402 established)."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT STARTED. #1252's body is explicit that enumerating idioms cannot terminate -- Site C (`find(\"://\")`) was missed by exactly that alphabet. The set proven workable in the #1252 close sweep is every production `.rs` line under `crates/` carrying the literal `\"://\"`: 24 hits, a superset of every idiom, and it does catch `find(\"://\")`."
  - id: c3
    text: "Every site the enumeration finds is classified -- `answers through the parser`, `returns no host`, or `renders without deciding` -- with the reason recorded where the gate reads it, so the already-dispositioned display cuts and `events.rs::split_endpoint` stay green without weakening it."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT STARTED. The classification itself already exists as prose in .planning/ledger/wayland-1252.md's verification block; what is missing is putting it where a gate reads it, the way wayland-core#402's RESOLVER INVENTORY block does for workspace_policy.rs."
  - id: c4
    text: "The three sites #1252 fixed and the two #1243 fixed are all seen by the enumeration -- the known-positive control, so a gate that finds nothing cannot pass."
    state: not-met
    evidence: ""
    owner: core
    note: "NOT STARTED. Without this control an enumeration that silently matches nothing passes, which is the failure mode an empty query always has."

---

Created 2026-08-31 by lane f13-authority, which found it while closing
wayland#1252 and declined to grade #1252 c3 on it: c3 as written is a property
of two named sites and it holds, and reading a repo-wide gate obligation into it
would have been grading an adjacent property in the other direction.

`#1211` -> `#1243` -> `#1252` is the same class arriving three times, each found
by a hand sweep AFTER the fact, and #1252's own Site C was missed by the sweep
before it. The inversion #1252's body asks for was performed as a MEASUREMENT
for that close and found nothing new -- the full 24-hit reading is recorded in
.planning/ledger/wayland-1252.md. A measurement is a snapshot; a fourth cut is
caught by whoever next thinks to look.

No red arm is attached: the defect is a counterfactual about a cut that does not
exist yet, and manufacturing one would report a modelled failure as a measured
one. Same posture as wayland-core#402's own filing, which this ticket is the
authority-parser twin of.
