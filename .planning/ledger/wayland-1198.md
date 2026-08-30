---
issue: 1198
repo: FerroxLabs/wayland
kind: defect
title: "check-criteria-ledger.py cannot detect drift in a file:<path>:<line> anchor, which is the drift it exists to catch"
status: open
last_verified_commit: da0c8a31
criteria:
  - id: c1
    text: "A file: anchor carries a required content fragment that must be present at or near the named line, or bare line anchors are refused on files above a stated size"
    state: met
    evidence: "symbol:scripts/check-criteria-ledger.py::_resolve_file"
    owner: core
    note: "Implements the FIRST disjunct in its strongest form -- the fragment is REQUIRED, so the bare form is refused outright rather than only above a size, and no size threshold is needed. `file:<path>:<line>:<text>` now checks three things, each closing a different vacuity: the text is in the file at all (else the evidence is gone, not moved); it occurs EXACTLY ONCE (a `);` or `}}` matches within a few lines of anywhere in a 39k-line file, so it reads like an anchor and pins nothing -- this rule caught four of my own conversions that would otherwise have passed while pinning nothing); and that one occurrence is within ANCHOR_WINDOW=20 lines of the cited line, the failure naming the line it moved TO so re-anchoring is a one-token edit. MEASURED: at base commit 1798076fe the offline gate exited 0 with 30 live bare line anchors; with this function it exits 1 naming all 30, which is the observable that would be absent if the change did nothing."
  - id: c2
    text: "Both #1134 anchors are re-anchored to something that resolves to what the criterion claims"
    state: met
    evidence: "absent:.planning/ledger/wayland-1134.md::.github/workflows/ci.yml:18"
    owner: core
    note: "The two wrong anchors were ci.yml:1806 and ci.yml:1888; both start `ci.yml:18` and neither replacement does, so this ONE token goes red if either comes back, which is what makes it the single evidence for a criterion about two anchors. What it proves is the removal; what proves the replacements RESOLVE is the gate itself, which now refuses a bare line and refuses a fragment that is missing, duplicated or out of window. Re-anchored to: c1 -> ci.yml:2041, the LIB leg's floor branch (`Executed $total tests, expected at least $MIN`), which is the `floored so it cannot pass while scanning nothing` half a step-name anchor would not prove; c2 -> ci.yml:2116, `done < <(python3 scripts/check-test-env-globals.py --shared-process-targets)`, where `over the targets that touch process globals` is actually decided. VERIFIED AGAINST THE OLD ANCHORS: 1806 was a bare `#` in the retry-evidence comment ~230 lines above the lib step, and 1888 was inside the swarm delegated-dispatch filterset -- a different step, and the wrong one of the two legs c2 distinguishes, exactly as the issue states. ONE FRAGILITY, stated rather than left to be found: this token is a text absence over this file, so spelling either OLD anchor out in full -- the workflow path followed by 1806 or 1888 -- anywhere in this entry would red it. The re-anchor notes above use the short ci.yml:NNNN form for exactly that reason, and a later editor must keep doing so."
  - id: c3
    text: "--self-test proves both directions: an anchor whose content moved goes red, and a correct anchor stays green"
    state: met
    evidence: "file:scripts/check-criteria-ledger.py:1159:file anchor: content ONE line past the window has drifted"
    owner: core
    note: "The moved-content arm. Its paired green arm is at :985 and the two differ by ONE line -- the fragment sits at the far edge of the window in the green arm and one line past it in the red one -- so the pair proves the window is neither zero (which would red on any edit above an anchor) nor unbounded (which would make the line number decorative). Seven more file: arms land beside them, each reddened by a DIFFERENT property so none rides on another`s coverage: content present, content unique, content in window, fragment length, the bare-line refusal itself, past-EOF, and `file:<path>` with no line staying green. The harness`s own anti-vacuity guard was widened in the same change: it refused an unchanged fixture only for RED arms, which left every GREEN arm able to pass as a second copy of the clean control; arms that deliberately do not mutate now say so by passing _ident, and every other arm is checked."
---

`scripts/check-criteria-ledger.py` accepted `file:<path>:<line>` evidence on a
line-count check alone -- "the file exists and has at least <line> lines" -- so
any number below the file`s length passed forever and the anchor rotted the
moment anyone edited above it. In the one gate whose whole purpose is to catch
a `met` claim drifting away from its evidence, `file:` anchors could not detect
drift at all.

A line anchor now carries the content that line is supposed to hold, and the
bare form is refused with a message that hands the reader the conversion,
including what the cited line currently says.

**The 30 live anchors at this commit were all converted**, and the conversion
was not mechanical: four fragments copied straight off the anchored line were
rejected by the uniqueness rule, and eleven anchors turned out to be pointing
somewhere unrelated to their own criterion -- #1166 c4 had drifted 351 lines
onto the closing paren of a tracing macro, and the note recording its previous
re-anchor was itself already stale. Those were re-derived from each entry`s own
prose and re-verified against the code, not guessed at.

**Sibling lanes are landing ledger entries with bare line anchors while this
change is in flight.** Every one of them will red the offline gate on merge and
needs the same one-token conversion; the failure message names the file, the
line and what that line currently reads, so the pass is mechanical.
