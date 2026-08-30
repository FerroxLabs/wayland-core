#!/usr/bin/env python3
"""Fail when the criteria ledger and reality have drifted apart.

WHY THIS EXISTS
    Handoffs in this repo are narratives of what was DONE. They are not
    ledgers of what is TRUE, so every session re-derives "is this issue
    done?" from prose and gets a different answer. v0.13.10 shipped claiming
    22 issues closed; grading them against their own stated criteria found 9.

    The single largest contributor was structural, not sloppy: the sweep that
    produced 0.13.9 filtered `FerroxLabs/wayland` on `area:core`, and an
    ENTIRE SECOND TRACKER (`FerroxLabs/wayland-core`, 17 open issues) was
    invisible for a full release. Nothing went red. Nothing could have.

    So the first-class check here is COVERAGE of both trackers, and the
    second is that a `met` criterion's evidence still RESOLVES in the tree.
    A ledger that can be wrong without going red is worse than no ledger --
    it manufactures confidence.

WHAT A LEDGER FILE IS
    `.planning/ledger/<repo>-<number>.md` -- one per issue, either tracker.
    Strict single-line YAML-subset frontmatter, then prose for humans:

        ---
        issue: 934
        repo: FerroxLabs/wayland
        title: "max_message_len is unverified across 8 adapters"
        status: open
        last_verified_commit: cfa89a9c
        criteria:
          - id: c1
            text: "caps are asserted against a real boundary, not themselves"
            state: met
            evidence: "test:crates/x/tests/caps.rs::the_boundary_is_probed"
            owner: core
            note: "optional free text"
        ---

    `state` is `met` | `not-met` | `blocked` | `superseded`. `superseded` is
    for a residual that was deliberately handed to another issue when this one
    closed -- its `note` MUST name that issue by `#<number>`, and that issue
    must exist and still be open. It is the only way to close an issue with a
    known remainder without the remainder disappearing.

    `evidence` is ONE machine-resolvable token, never prose:
        test:<path>::<test name>   file exists AND declares `fn <name>`
        symbol:<path>::<name>      file exists AND declares that item
        file:<path>:<line>:<text>  file exists AND <text> occurs EXACTLY once
                                   in it, within ANCHOR_WINDOW lines of <line>
        file:<path>                file exists
        commit:<sha>               resolves to a commit object
        absent:<path>::<text>      file exists AND does NOT contain <text>

    Prefer `test:` and `symbol:` over any `file:` anchor. `file:` with a line
    is for evidence that is genuinely positional -- a workflow step, a table
    row, a floor inside a shell block -- and the prose should say what is
    there.

    A BARE `file:<path>:<line>` IS REFUSED. It used to be accepted on a
    line-count check -- "the file exists and has at least <line> lines" -- so
    ANY number below the file's length passed forever, and the anchor rotted
    silently the moment anybody edited above it. That is not a weak drift
    check; it is no drift check at all, in the one gate whose entire purpose
    is to catch drift. Measured on FerroxLabs/wayland#1134, which is what
    produced #1198: `ci.yml:1806` was recorded for "a shared-process LIB leg
    runs in CI, floored" and landed on a bare `#` inside an unrelated
    retry-evidence comment ~230 lines above that step; `ci.yml:1888` was
    recorded for the INTEGRATION leg and landed inside the swarm
    delegated-dispatch filterset, a different step again. Both read green, and
    one was already wrong at the commit the entry records as last_verified.

    So a line anchor now carries the CONTENT that line is supposed to hold,
    and three separate ways of being vacuous are each closed:
      * the text is in the file AT ALL -- otherwise the evidence is gone
        rather than merely displaced, and the claim needs re-verifying, not
        re-anchoring.
      * it occurs EXACTLY ONCE. A fragment like `);` or `}}` matches within a
        few lines of anywhere in a 39k-line file, so it can never register a
        move -- it reads like an anchor and pins nothing.
      * that one occurrence is within ANCHOR_WINDOW lines of <line>. Otherwise
        the recorded position is stale; the failure names the line the content
        moved TO, so re-anchoring is a one-token edit.
    `absent:` is for a criterion whose whole content is that something is
    GONE -- a deleted allowlist entry, a removed flag, a retired code path.
    `commit:<sha>` is the wrong anchor for those: it proves a deletion once
    HAPPENED and stays green after a later merge resolution puts the line
    back. That is not hypothetical. FerroxLabs/wayland#1182 c3 recorded
    `commit:c461293f` for a flaky-allowlist deletion; merge 9c9f27b0 restored
    the line from the other side of the resolution; the criterion went on
    reading `met` over a file that still had the entry in it, and `git log -S`
    did not show the resurrection because it skips merges by default.
    `absent:` re-reads the file on every run, so a resurrection reds the gate
    instead of surviving it.

    The path must EXIST, and that is the known-positive control: an absence
    check over a renamed or deleted file fails loudly rather than passing,
    because an empty result reads exactly like the thing being gone.

    A criterion needing two pieces of evidence is two criteria. That is
    deliberate: "evidence: see the PR" is how a ledger rots into a narrative.

WHAT IT FAILS ON
    * an open issue in EITHER tracker with no ledger file            (coverage)
    * `met` with no evidence, or evidence that does not resolve      (rot)
    * `blocked` owned by `core` -- core cannot block on itself
    * `blocked` with no stated reason -- that is a suppression
    * `superseded` whose `note` names no successor issue, or names one that
      does not exist or is already CLOSED. Closing an issue with a residual
      is legitimate; leaving the residual untracked is not, and "tracked in
      a closed issue" is untracked with extra steps.
    * ledger says every criterion met but GitHub still has it open,
      or the reverse                                              (divergence)
    * a ledger file for an issue that does not exist in its tracker  (typo)
    * a malformed / unparseable ledger file
    * scanning nothing: zero files, zero criteria, or a tracker query
      that reached nothing while the tracker demonstrably has issues

WHAT IT DOES NOT DO
    It does not judge whether a criterion is a GOOD criterion, and it cannot
    tell a true `met` from a plausible one. It checks that the claim is
    ANCHORED -- that something named still exists. Deleting the test a `met`
    criterion cites turns that criterion red, which is the specific rot this
    was built to catch.

    python3 scripts/check-criteria-ledger.py --self-test   # prove both directions
    python3 scripts/check-criteria-ledger.py               # the gate (needs gh)
    python3 scripts/check-criteria-ledger.py --offline     # structure only
"""
import json
import os
import re
import subprocess
import sys
import tempfile

LEDGER_DIR = os.path.join(".planning", "ledger")

# The trackers. `label` narrows a tracker that also serves other lanes; None
# means every open issue in that repo is in scope. Adding a tracker here is
# what makes it visible -- which is exactly the step that was never taken for
# wayland-core, so it is one line and it is in the source, not in a workflow.
TRACKERS = [
    ("FerroxLabs/wayland", "area:core"),
    ("FerroxLabs/wayland-core", None),
]

STATES = ("met", "not-met", "blocked", "superseded")
OWNERS = ("core", "desktop", "flux", "maintainer", "reporter")
STATUSES = ("open", "closed")

TOP_KEYS = {"issue", "repo", "title", "status", "last_verified_commit", "criteria"}
CRIT_KEYS = {"id", "text", "state", "evidence", "owner", "note", "handoff"}
# Keys this schema ALLOWS but does not require, and does not itself judge.
# `kind: defect|feature` and a criterion's `handoff:` belong to
# scripts/check-release-readiness.py, which decides whether an issue is
# release-blocking and whether a remainder handed to another lane is still
# tracked once it leaves this lane. They are listed here only so this parser
# does not reject a field it has no opinion about -- the allowlist is strict
# on purpose, and a second parser for the same frontmatter is two grammars
# that drift. Nothing this gate fails on changes: `kind` is required by the
# release gate, not by this one, and `handoff` is judged there too.
TOP_OPTIONAL = {"kind"}
SUCCESSOR = re.compile(r"#(\d+)")
CRIT_REQUIRED = {"id", "text", "state", "owner"}


# ── frontmatter parsing ──────────────────────────────────────────────────────


def _scalar(raw, where, errs):
    """One single-line scalar. Quoted or bare. Anything else is malformed.

    Deliberately NOT a YAML parser. The schema is fixed and small, and a real
    YAML load would happily accept a block scalar, an anchor or a nested map
    that no reader of this file expects -- silently widening what a ledger can
    say. Narrow and strict beats permissive here.
    """
    v = raw.strip()
    if not v:
        errs.append("%s: empty value" % where)
        return ""
    if v[0] in "\"'":
        q = v[0]
        if len(v) < 2 or v[-1] != q:
            errs.append("%s: unterminated %s-quoted value" % (where, q))
            return v.strip(q)
        return v[1:-1]
    if v[0] in "[{|>&*":
        errs.append(
            "%s: %r starts a YAML structure this schema does not allow. Values "
            "are single-line scalars; a criterion needing two things is two "
            "criteria." % (where, v[0])
        )
        return ""
    return v


def parse_ledger(path):
    """-> (record, [errors]). Never raises: a broken file is a FINDING."""
    errs = []
    try:
        text = open(path, encoding="utf-8").read()
    except OSError as e:
        return None, ["%s: unreadable (%s)" % (path, e)]

    lines = text.split("\n")
    if not lines or lines[0].strip() != "---":
        return None, ["%s: does not open with a `---` frontmatter fence" % path]
    try:
        end = next(i for i in range(1, len(lines)) if lines[i].strip() == "---")
    except StopIteration:
        return None, ["%s: frontmatter is never closed by a second `---`" % path]

    body = "\n".join(lines[end + 1 :]).strip()
    rec = {"path": path, "criteria": [], "prose": body}
    seen_top = set()
    in_criteria = False
    cur = None
    order = []

    for n, line in enumerate(lines[1:end], start=2):
        where = "%s:%d" % (path, n)
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        indent = len(line) - len(line.lstrip())

        if indent == 0:
            in_criteria = False
            cur = None
            if ":" not in line:
                errs.append("%s: not a `key: value` line" % where)
                continue
            k, _, v = line.partition(":")
            k = k.strip()
            if k not in TOP_KEYS | TOP_OPTIONAL:
                errs.append("%s: unknown key %r (allowed: %s)"
                            % (where, k,
                               ", ".join(sorted(TOP_KEYS | TOP_OPTIONAL))))
                continue
            if k in seen_top:
                errs.append("%s: duplicate key %r" % (where, k))
            seen_top.add(k)
            if k == "criteria":
                if v.strip():
                    errs.append("%s: `criteria:` takes a list on following "
                                "lines, not an inline value" % where)
                in_criteria = True
            else:
                rec[k] = _scalar(v, where, errs)
            continue

        if not in_criteria:
            errs.append("%s: indented line outside `criteria:`" % where)
            continue

        s = line.strip()
        if s.startswith("- "):
            cur = {"_line": n}
            order.append(cur)
            rec["criteria"].append(cur)
            s = s[2:]
        elif cur is None:
            errs.append("%s: criteria entry must start with `- `" % where)
            continue
        if ":" not in s:
            errs.append("%s: not a `key: value` line" % where)
            continue
        k, _, v = s.partition(":")
        k = k.strip()
        if k not in CRIT_KEYS:
            errs.append("%s: unknown criterion key %r (allowed: %s)"
                        % (where, k, ", ".join(sorted(CRIT_KEYS))))
            continue
        if k in cur:
            errs.append("%s: duplicate criterion key %r" % (where, k))
        cur[k] = _scalar(v, where, errs)

    for k in sorted(TOP_KEYS):
        if k not in seen_top:
            errs.append("%s: missing required key %r" % (path, k))
    if not rec["criteria"]:
        errs.append("%s: no criteria. A ledger entry with nothing to verify "
                    "cannot fail, so it is not an entry." % path)
    return rec, errs


# ── validation of one record ─────────────────────────────────────────────────


TEST_EV = re.compile(r"^test:(?P<p>[^:]+(?::[^:]+)*?)::(?P<n>[A-Za-z0-9_]+)$")
SYM_EV = re.compile(r"^symbol:(?P<p>[^:]+(?::[^:]+)*?)::(?P<n>[A-Za-z0-9_]+)$")
DECL = r"\b(?:fn|struct|enum|union|const|static|type|trait|mod|def|class|macro_rules!)\s+%s\b"
# Tried in this order. The line form is matched before the bare-path form so
# a trailing `:1806` is read as a position and refused, not silently swallowed
# into a path that then fails as "no such file" for the wrong reason.
FILE_FRAG_EV = re.compile(r"^file:(?P<p>.+?):(?P<l>\d+):(?P<frag>.+)$")
FILE_LINE_EV = re.compile(r"^file:(?P<p>.+?):(?P<l>\d+)$")
FILE_EV = re.compile(r"^file:(?P<p>.+)$")

# How far the anchored CONTENT may sit from the line the ledger names before
# the anchor counts as stale. Not zero: an edit inside the same block shifts a
# line by a few without touching what the entry claims, and a zero window
# would red the gate on every unrelated insertion above it -- which trains
# people to widen the window rather than fix the anchor. Not large either: the
# unit a positional anchor names -- a workflow step, a shell floor, a table
# row -- is smaller than this, and two consecutive ones are not, so a hit this
# close is still the place the entry meant.
ANCHOR_WINDOW = 20

# The shortest fragment that can pin anything. This is the floor under a
# MECHANICAL conversion of an old bare anchor: at wayland#1198 the line under
# four of the thirty live anchors was blank, `);`, `}},` or `#`, and copying
# that out as the fragment would have reproduced the defect being fixed with
# extra steps. Uniqueness rejects most of them; this rejects the rest without
# needing the file to be long enough for a duplicate to exist.
MIN_FRAGMENT = 3
COMMIT_EV = re.compile(r"^commit:(?P<s>[0-9a-f]{7,40})$")
ABSENT_EV = re.compile(r"^absent:(?P<p>[^:]+(?::[^:]+)*?)::(?P<n>.+)$")
SLUG = re.compile(r"^(?P<slug>[a-z0-9][a-z0-9-]*?)-(?P<num>\d+)\.md$")


def _is_git(root):
    return subprocess.run(
        ["git", "-C", root, "rev-parse", "--git-dir"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    ).returncode == 0


def _is_shallow(root):
    """True when only part of the history is present.

    actions/checkout defaults to `fetch-depth: 1`, so in CI the ONLY commit
    that resolves is HEAD. Every ledger file carries a `last_verified_commit`
    by construction, so validating those shas against a shallow clone produces
    one guaranteed problem per file -- 63 of them here -- and the step can
    never pass on any branch or any tree. A gate with no reachable pass state
    is worth exactly as much as one that cannot fail, and it takes the real
    checks in the same step down with it.

    So the sha check DOWNGRADES to a named skip when the clone is shallow,
    rather than being silently dropped: the run says which check did not run
    and why, and the same gate stays fully armed on a full clone.
    """
    return subprocess.run(
        ["git", "-C", root, "rev-parse", "--is-shallow-repository"],
        capture_output=True, text=True,
    ).stdout.strip() == "true"


def _resolve_file(root, path, line, frag):
    """`file:` evidence. See the module docstring for why a bare line is out.

    Returns None when the anchor holds, else the reason it does not -- phrased
    so the reader can fix it without opening this script, because the whole
    cost of tightening an anchor grammar lands on whoever hits the message.
    """
    p = os.path.join(root, path)
    if not os.path.isfile(p):
        return "no such file: %s" % path
    if line is None:
        return None
    line = int(line)
    lines = open(p, encoding="utf-8", errors="replace").read().split("\n")
    if lines and lines[-1] == "":
        lines.pop()  # a trailing newline terminates a line, it does not add one
    n = len(lines)
    if line > n:
        return "%s has %d lines; evidence cites line %d" % (path, n, line)
    here = lines[line - 1].strip()

    if frag is None:
        return (
            "%s:%d is a BARE line anchor. A line number on its own is a "
            "position, not a claim: the only thing checkable about it is that "
            "the file is that long, so every number below its length passed "
            "forever while the anchor rotted (FerroxLabs/wayland#1198). Carry "
            "the content as well -- file:%s:%d:<a substring that occurs "
            "exactly once in the file>. Line %d reads: %r"
            % (path, line, path, line, line, here))

    if len(frag.strip()) < MIN_FRAGMENT:
        return ("%s:%d anchors on %r, which is too short to pin anything. Use "
                "at least %d non-space characters of what is actually there. "
                "Line %d reads: %r"
                % (path, line, frag, MIN_FRAGMENT, line, here))

    hits = [i + 1 for i, t in enumerate(lines) if frag in t]
    if not hits:
        return ("%s does not contain %r anywhere -- the evidence is gone, not "
                "merely moved, so re-verify the claim before re-anchoring it. "
                "Line %d now reads: %r" % (path, frag, line, here))
    if len(hits) > 1:
        shown = ", ".join(str(h) for h in hits[:6])
        return ("%s contains %r on %d lines (%s%s). A fragment that matches "
                "more than once pins nothing: a near-enough hit turns up "
                "beside almost any line, so the anchor can never register a "
                "move. Lengthen it until it is unique."
                % (path, frag, len(hits), shown,
                   ", ..." if len(hits) > 6 else ""))

    got = hits[0]
    if abs(got - line) > ANCHOR_WINDOW:
        return ("%s: the anchored content has MOVED -- cited at line %d, it is "
                "now at line %d (%+d, window is +/-%d). The claim may well "
                "still hold; the position recorded for it does not. Re-anchor "
                "to file:%s:%d:%s"
                % (path, line, got, got - line, ANCHOR_WINDOW, path, got, frag))
    return None


def resolve_evidence(root, ev, git, shallow=False):
    """-> None if it resolves, else why it does not."""
    m = TEST_EV.match(ev)
    if m:
        p = os.path.join(root, m.group("p"))
        if not os.path.isfile(p):
            return "no such file: %s" % m.group("p")
        t = open(p, encoding="utf-8", errors="replace").read()
        if not re.search(r"\bfn\s+%s\s*[(<]" % re.escape(m.group("n")), t):
            return "%s declares no `fn %s`" % (m.group("p"), m.group("n"))
        return None
    m = SYM_EV.match(ev)
    if m:
        p = os.path.join(root, m.group("p"))
        if not os.path.isfile(p):
            return "no such file: %s" % m.group("p")
        t = open(p, encoding="utf-8", errors="replace").read()
        if not re.search(DECL % re.escape(m.group("n")), t):
            return "%s declares no `%s`" % (m.group("p"), m.group("n"))
        return None
    m = FILE_FRAG_EV.match(ev) or FILE_LINE_EV.match(ev) or FILE_EV.match(ev)
    if m:
        g = m.groupdict()
        return _resolve_file(root, g["p"], g.get("l"), g.get("frag"))
    m = ABSENT_EV.match(ev)
    if m:
        p = os.path.join(root, m.group("p"))
        if not os.path.isfile(p):
            # The control. An absence check over a path that is not here would
            # otherwise pass forever on a typo or a rename -- an empty result
            # reads exactly like the thing being gone.
            return ("no such file: %s (an absence cannot be verified against "
                    "a file that is not here)" % m.group("p"))
        t = open(p, encoding="utf-8", errors="replace").read()
        if m.group("n") in t:
            return "%s still contains %r" % (m.group("p"), m.group("n"))
        return None
    m = COMMIT_EV.match(ev)
    if m:
        if not git:
            return "commit evidence needs a git worktree; this root is not one"
        if shallow:
            # Same shallow wall as `last_verified_commit` -- see _is_shallow.
            # Fixing only that site would have left this one to surface later;
            # the class is "any sha resolved through git", not one field.
            return None
        r = subprocess.run(
            ["git", "-C", root, "cat-file", "-t", m.group("s")],
            capture_output=True, text=True)
        if r.returncode != 0 or r.stdout.strip() != "commit":
            return "%s does not resolve to a commit in this tree" % m.group("s")
        return None
    return ("%r is not a machine-resolvable evidence token. Use "
            "test:<path>::<name>, symbol:<path>::<name>, "
            "file:<path>:<line>:<content>, file:<path>, "
            "absent:<path>::<text> or commit:<sha>." % ev)


def validate_record(root, rec, git, shallow=False):
    """-> list of complaints about one ledger record."""
    bad = []
    p = rec["path"]
    base = os.path.basename(p)
    m = SLUG.match(base)
    if not m:
        bad.append("%s: filename must be `<repo>-<number>.md`" % p)

    repo = rec.get("repo", "")
    if repo not in [t for t, _ in TRACKERS]:
        bad.append("%s: repo %r is not a known tracker (%s)"
                   % (p, repo, ", ".join(t for t, _ in TRACKERS)))
    elif m and m.group("slug") != repo.split("/")[-1]:
        bad.append("%s: filename slug %r does not match repo %r"
                   % (p, m.group("slug"), repo))

    num = rec.get("issue", "")
    if not str(num).isdigit():
        bad.append("%s: issue %r is not a number" % (p, num))
    elif m and m.group("num") != str(num):
        bad.append("%s: filename says issue %s, frontmatter says %s"
                   % (p, m.group("num"), num))

    if rec.get("status") not in STATUSES:
        bad.append("%s: status %r must be one of %s"
                   % (p, rec.get("status"), "/".join(STATUSES)))
    if not rec.get("title", "").strip():
        bad.append("%s: empty title" % p)
    sha = rec.get("last_verified_commit", "")
    if not re.fullmatch(r"[0-9a-f]{7,40}", sha or ""):
        bad.append("%s: last_verified_commit %r is not a sha" % (p, sha))
    elif git and not shallow:
        # ANCESTRY, not existence. `cat-file -t` only asks whether the object is
        # in this repo, and a working checkout accumulates objects from every
        # lane branch it has ever fetched. So a sha that is on NO branch at all
        # -- a remnant of a rebased or abandoned lane -- resolves locally and is
        # absent in CI, which checks out only the branch under test.
        #
        # Measured 2026-08-30: twelve entries cited `be4467ed`. `cat-file -t`
        # said `commit` on hetzner, `git branch -a --contains` listed NOTHING,
        # and CI failed all twelve. The local gate was weaker than the CI gate
        # in exactly the direction that lets a bad pointer ship.
        #
        # A `last_verified_commit` names the tree an entry was graded against.
        # If it is not reachable from HEAD, that tree is not this one and the
        # grading cannot be re-derived by anyone reading the release.
        r = subprocess.run(["git", "-C", root, "cat-file", "-t", sha],
                           capture_output=True, text=True)
        if r.stdout.strip() != "commit":
            bad.append("%s: last_verified_commit %s is not a commit in this "
                       "tree -- the entry was verified against something that "
                       "is not here" % (p, sha))
        elif subprocess.run(["git", "-C", root, "merge-base",
                             "--is-ancestor", sha, "HEAD"]).returncode != 0:
            bad.append("%s: last_verified_commit %s is not an ANCESTOR of HEAD "
                       "-- the object exists in this checkout (a fetched lane "
                       "branch, or a remnant of a rebased one) but is not in "
                       "the history being shipped, so nobody reading the "
                       "release can re-derive the grading" % (p, sha))
    if len(rec.get("prose", "").strip()) < 40:
        bad.append("%s: no prose body. The file must be readable by a human "
                   "with no context; frontmatter alone is not." % p)

    ids = set()
    for c in rec["criteria"]:
        w = "%s:%d" % (p, c.get("_line", 0))
        for k in sorted(CRIT_REQUIRED):
            if k not in c:
                bad.append("%s: criterion missing %r" % (w, k))
        cid = c.get("id", "")
        if cid in ids:
            bad.append("%s: duplicate criterion id %r" % (w, cid))
        ids.add(cid)
        if not c.get("text", "").strip():
            bad.append("%s: empty criterion text" % w)
        st = c.get("state")
        if st not in STATES:
            bad.append("%s: state %r must be one of %s"
                       % (w, st, "/".join(STATES)))
        own = c.get("owner")
        if own not in OWNERS:
            bad.append("%s: owner %r must be one of %s"
                       % (w, own, "/".join(OWNERS)))
        ev = c.get("evidence", "").strip()

        if st == "met" and not ev:
            bad.append("%s: %s is `met` with no evidence. An unevidenced "
                       "`met` is the claim this ledger exists to replace."
                       % (w, cid))
        if st == "blocked":
            if own == "core":
                bad.append("%s: %s is `blocked` owned by `core`. Core cannot "
                           "block on itself -- name the lane that owes the "
                           "work, or mark it not-met." % (w, cid))
            if len(c.get("note", "").strip()) < 20:
                bad.append("%s: %s is `blocked` with no stated reason. A "
                           "blocked criterion without a `note` saying what is "
                           "being waited on is a suppression." % (w, cid))
        if st == "superseded":
            if not SUCCESSOR.search(c.get("note", "")):
                bad.append(
                    "%s: %s is `superseded` but its note names no successor "
                    "issue. A residual nobody can find is a residual nobody "
                    "will fix -- put the `#<number>` that carries it in the "
                    "note." % (w, cid))
        if ev:
            why = resolve_evidence(root, ev, git, shallow)
            if why:
                bad.append("%s: %s evidence does not resolve -- %s" % (w, cid, why))
    return bad


# ── tracker state ────────────────────────────────────────────────────────────


class TrackerError(Exception):
    pass


# High enough that no tracker here is near it, and ASSERTED rather than hoped:
# `gh issue list` silently truncates at --limit and returns exactly that many
# rows with no error. At 500 it truncated FerroxLabs/wayland on the first live
# run of this gate and reported two genuinely-open issues as ORPHANs, which is
# the same shape of quiet wrong answer the ledger exists to stop.
GH_LIMIT = 4000


def gh_issues(repo, label, state):
    args = ["gh", "issue", "list", "-R", repo, "--state", state,
            "--limit", str(GH_LIMIT), "--json", "number,title,state"]
    if label:
        args += ["--label", label]
    try:
        r = subprocess.run(args, capture_output=True, text=True)
    except OSError as e:
        raise TrackerError(
            "%s: could not run `gh` (%s). The coverage check needs it; pass "
            "--offline if you meant to skip coverage, and read what --offline "
            "prints before calling the result a pass." % (repo, e))
    if r.returncode != 0:
        raise TrackerError("%s: `gh issue list` failed -- %s"
                           % (repo, (r.stderr or "").strip()[:300]))
    try:
        rows = json.loads(r.stdout)
    except ValueError as e:
        raise TrackerError("%s: unparseable gh output (%s)" % (repo, e))
    if len(rows) >= GH_LIMIT:
        raise TrackerError(
            "%s: the %s query returned exactly %d rows, which is the request "
            "limit -- it was TRUNCATED, and a truncated tracker reads as a "
            "tracker with fewer issues than it has. Raise GH_LIMIT."
            % (repo, state, GH_LIMIT))
    return rows


def load_trackers(injected):
    """-> ({repo: {num: state}}, {repo: set(open in-scope nums)}, [notes]).

    `injected` short-circuits the network for --self-test's fixtures. It has
    no command-line switch on purpose -- see main().
    """
    if injected is not None:
        return injected["all"], {k: set(v) for k, v in injected["scoped"].items()}, []
    allstate, scoped, notes = {}, {}, []
    for repo, label in TRACKERS:
        op = gh_issues(repo, label, "open")
        every = gh_issues(repo, None, "all")
        if not every:
            raise TrackerError(
                "%s: the tracker query reached ZERO issues in any state. That is "
                "not a clean tracker, it is a broken query -- and a tracker "
                "nobody can see is exactly how wayland-core went missing for a "
                "release." % repo)
        if not op:
            notes.append("%s: no open in-scope issues. The tracker IS reachable "
                         "(%d issues in all states), so this is a real zero."
                         % (repo, len(every)))
        allstate[repo] = {int(i["number"]): i["state"].lower() for i in every}
        scoped[repo] = {int(i["number"]) for i in op}
        # Belt and braces against the same class: whatever the all-states query
        # says, an issue the OPEN query just returned is open and exists.
        for i in op:
            allstate[repo][int(i["number"])] = "open"
    return allstate, scoped, notes


# ── the gate ─────────────────────────────────────────────────────────────────


def collect(root):
    d = os.path.join(root, LEDGER_DIR)
    if not os.path.isdir(d):
        return None
    return sorted(os.path.join(d, f) for f in os.listdir(d) if f.endswith(".md"))


def run(root, offline=False, injected=None, quiet=False):
    """-> (exit code, [lines])."""
    out = []

    def say(s=""):
        out.append(s)

    git = _is_git(root)
    shallow = git and _is_shallow(root)
    if shallow:
        # Say it. A check that quietly stops running is indistinguishable from
        # one that ran and passed, and that is how a gate rots between releases.
        say("NOTE: shallow clone -- `last_verified_commit` resolution is SKIPPED "
            "for every entry. Only HEAD resolves at fetch-depth 1, so the check "
            "could only ever report one problem per ledger file. Every OTHER "
            "ledger check below still ran. Set `fetch-depth: 0` on the checkout "
            "to arm it.")
    files = collect(root)
    if files is None:
        say("FAIL: %s does not exist. There is no ledger to check, which is not "
            "the same as a clean one." % LEDGER_DIR)
        return 2, out
    if not files:
        say("FAIL: %s holds no ledger files. A gate that scans nothing cannot "
            "fail, and this repo has shipped one of those before." % LEDGER_DIR)
        return 2, out

    records, problems = [], []
    for f in files:
        rec, errs = parse_ledger(f)
        problems += errs
        if rec is not None:
            records.append(rec)

    ncrit = sum(len(r["criteria"]) for r in records)
    if ncrit == 0:
        say("FAIL: %d ledger file(s) and zero criteria between them." % len(files))
        return 2, out

    for rec in records:
        problems += validate_record(root, rec, git, shallow)

    counts = {s: 0 for s in STATES}
    for r in records:
        for c in r["criteria"]:
            if c.get("state") in counts:
                counts[c["state"]] += 1

    say("ledger files: %d   criteria: %d   met %d / not-met %d / blocked %d "
        "/ superseded %d"
        % (len(files), ncrit, counts["met"], counts["not-met"],
           counts["blocked"], counts["superseded"]))
    # The summary must add up. A criterion whose state this gate does not
    # recognise is already a per-criterion complaint below; printing the
    # residue here as well stops a whole class hiding inside a total that
    # silently does not sum.
    if sum(counts.values()) != ncrit:
        say("   ...and %d criterion(s) whose state is not one of those -- see "
            "the per-criterion complaints below."
            % (ncrit - sum(counts.values())))
    say("evidence tokens resolved against the working tree at %s"
        % (os.path.abspath(root)))
    if not git:
        say("NOTE: not a git worktree -- commit-shaped evidence cannot be resolved.")

    if injected is not None:
        say("NOTE: tracker state was INJECTED (--issues-json), not fetched. "
            "This is a fixture run, not a gate run.")

    if offline:
        say()
        say("OFFLINE: tracker coverage and ledger/GitHub divergence were NOT "
            "checked. THIS IS NOT A PASS for coverage -- an entire tracker can "
            "be missing and this run would still be green. Re-run without "
            "--offline before believing the ledger is complete.")
    else:
        try:
            allstate, scoped, notes = load_trackers(injected)
        except TrackerError as e:
            say()
            say("FAIL: %s" % e)
            say("The gate refuses to degrade to a structural check silently. "
                "Pass --offline if you meant to skip coverage.")
            return 2, out
        for n in notes:
            say("NOTE: %s" % n)

        have = {(r.get("repo"), str(r.get("issue"))) for r in records}
        reached = sum(len(v) for v in allstate.values())
        if reached == 0:
            say("FAIL: reached zero issues across %d tracker(s)." % len(TRACKERS))
            return 2, out
        say("trackers: %s -- %d issue(s) reached, %d open and in scope"
            % (", ".join(r for r, _ in TRACKERS), reached,
               sum(len(v) for v in scoped.values())))

        for repo, nums in sorted(scoped.items()):
            for n in sorted(nums):
                if (repo, str(n)) not in have:
                    problems.append(
                        "COVERAGE: %s#%s is OPEN and in scope with no ledger "
                        "file (%s/%s-%s.md). This is the check that would have "
                        "caught an entire tracker going invisible."
                        % (repo, n, LEDGER_DIR, repo.split("/")[-1], n))

        for rec in records:
            repo, num = rec.get("repo"), rec.get("issue")
            if repo not in allstate or not str(num).isdigit():
                continue
            gh_state = allstate[repo].get(int(num))
            if gh_state is None:
                problems.append(
                    "ORPHAN: %s names %s#%s, which does not exist in that "
                    "tracker." % (rec["path"], repo, num))
                continue
            if rec.get("status") != gh_state:
                problems.append(
                    "DIVERGENCE: %s says status: %s; %s#%s is %s on GitHub."
                    % (rec["path"], rec.get("status"), repo, num, gh_state))
            for c in rec["criteria"]:
                if c.get("state") != "superseded":
                    continue
                m2 = SUCCESSOR.search(c.get("note", ""))
                if not m2:
                    continue
                n2 = int(m2.group(1))
                where = [(rp, st) for rp, m3 in allstate.items()
                         for nn, st in m3.items() if nn == n2]
                if not where:
                    problems.append(
                        "%s: %s is superseded into #%d, which exists in "
                        "neither tracker." % (rec["path"], c.get("id"), n2))
                elif all(st == "closed" for _, st in where):
                    problems.append(
                        "%s: %s is superseded into #%d, which is CLOSED. A "
                        "residual handed to a closed issue is not tracked; it "
                        "is lost." % (rec["path"], c.get("id"), n2))
            states = [c.get("state") for c in rec["criteria"]]
            if (states and all(s in ("met", "superseded") for s in states)
                    and gh_state == "open"):
                problems.append(
                    "DIVERGENCE: %s marks every criterion met, but %s#%s is "
                    "still open. Either the issue closes or a criterion is "
                    "not actually met." % (rec["path"], repo, num))
            if any(s in ("not-met", "blocked") for s in states) and gh_state == "closed":
                problems.append(
                    "DIVERGENCE: %s carries an unmet criterion, but %s#%s is "
                    "CLOSED. That is the failure this ledger exists to catch: "
                    "22 closed, 9 met." % (rec["path"], repo, num))

    if problems:
        say()
        say("FAIL: %d problem(s)." % len(problems))
        for p in problems:
            say("  " + p)
        return 1, out
    say()
    say("OK: every ledger file parses, every `met` criterion is anchored to "
        "something that still exists, and%s"
        % (" coverage was not checked (offline)." if offline
           else " both trackers are fully covered."))
    return 0, out


# ── self-test ────────────────────────────────────────────────────────────────


_CLEAN = """---
issue: 7
repo: FerroxLabs/wayland
title: "a control that must stay green"
status: open
last_verified_commit: %s
criteria:
  - id: c1
    text: "the boundary is probed rather than asserted against itself"
    state: met
    evidence: "test:src/t.rs::the_boundary_is_probed"
    owner: core
  - id: c2
    text: "the second half is not built yet"
    state: not-met
    owner: core
  - id: c3
    text: "the credentialled probe cannot run here"
    state: blocked
    owner: maintainer
    note: "needs a Slack workspace credential the core lane does not hold"
---

Prose a human with no context can read: this is the fixture control for the
ledger gate's own self-test, and it must stay green in every arm.
"""


def _t_rs():
    """The fixture source, and the line numbers of its anchors.

    Long enough that "within ANCHOR_WINDOW lines" is a real constraint rather
    than something a three-line file satisfies by accident. The line numbers
    are DERIVED from the text and never written down: a self-test for a
    positional-anchor gate that hardcoded its own positional anchors would be
    committing the exact defect the gate now refuses.
    """
    body = ["pub struct Boundary;",
            "#[test]",
            "fn the_boundary_is_probed() { assert!(true); }"]
    body += ["// filler a%03d" % i for i in range(1, 121)]
    uniq = len(body) + 1
    body.append("const ANCHORED_ONCE: u8 = 1;")
    body += ["// filler b%03d" % i for i in range(1, 121)]
    twice = len(body) + 1
    body.append("const ANCHORED_TWICE: u8 = 2;")
    body += ["// filler c%03d" % i for i in range(1, 121)]
    body.append("const ANCHORED_TWICE: u8 = 2;")
    return "\n".join(body) + "\n", uniq, twice


_T_RS, _UNIQ, _TWICE = _t_rs()


def _fixture(root, body, name="wayland-7.md", extra=None):
    d = os.path.join(root, LEDGER_DIR)
    os.makedirs(d, exist_ok=True)
    os.makedirs(os.path.join(root, "src"), exist_ok=True)
    open(os.path.join(root, "src", "t.rs"), "w").write(_T_RS)
    for cmd in (["init", "-q"], ["add", "-A"],
                ["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "f"]):
        subprocess.run(["git", "-C", root] + cmd,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    sha = subprocess.run(["git", "-C", root, "rev-parse", "--short=8", "HEAD"],
                         capture_output=True, text=True).stdout.strip()
    open(os.path.join(d, name), "w").write(body % sha if "%s" in body else body)
    for n, b in (extra or {}).items():
        open(os.path.join(d, n), "w").write(b % sha if "%s" in b else b)
    return root, sha


_INJ_CLEAN = {
    "all": {"FerroxLabs/wayland": {7: "open"},
            "FerroxLabs/wayland-core": {9: "open"}},
    "scoped": {"FerroxLabs/wayland": [7], "FerroxLabs/wayland-core": [9]},
}
_CORE_9 = """---
issue: 9
repo: FerroxLabs/wayland-core
title: "the second tracker, which is the one that went invisible"
status: open
last_verified_commit: %s
criteria:
  - id: c1
    text: "the second tracker is covered at all"
    state: not-met
    owner: core
---

Prose a human with no context can read: the second tracker exists and its
issues need ledger files exactly like the first tracker's.
"""


def _ident(b):
    return b


def self_test():
    cases = []

    def case(label, mutate, must_fire, offline=False, inj=_INJ_CLEAN, expect=None):
        # `expect` is not decoration. A red arm that fires for the WRONG reason
        # proves nothing about the check it was written for, and a mutation
        # that silently stops applying (one did, on the first run of this
        # file) reads as a passing gate. Every RED arm names its own message.
        cases.append((label, mutate, must_fire, offline, inj, expect))

    case("clean control, both trackers covered", _ident, False)
    case("control again, offline", _ident, False, offline=True)
    case("met criterion cites a test that does not exist",
         lambda b: b.replace("::the_boundary_is_probed\"",
                             "::a_test_that_was_deleted\"", 1), True,
         expect="declares no `fn a_test_that_was_deleted`")
    case("met criterion cites a file line past EOF",
         lambda b: b.replace('"test:src/t.rs::the_boundary_is_probed"',
                             '"file:src/t.rs:9000:pub struct Boundary"'), True,
         expect="evidence cites line 9000")

    # ── file: anchors, both directions (FerroxLabs/wayland#1198) ─────────────
    # These come in pairs on purpose. Each RED arm is reddened by ONE property
    # -- presence, uniqueness, position, length -- and has a GREEN arm next to
    # it differing only in that property, so no arm rides on another's
    # coverage and none of them can be satisfied by a checker that simply
    # refuses every file: anchor.
    def anchor(tok):
        return lambda b: b.replace('"test:src/t.rs::the_boundary_is_probed"',
                                   '"%s"' % tok)

    case("file anchor: content is on the line it names",
         anchor("file:src/t.rs:%d:const ANCHORED_ONCE" % _UNIQ), False)
    case("file anchor: content is gone from the file entirely",
         anchor("file:src/t.rs:%d:const ANCHORED_ONCE_BUT_DELETED" % _UNIQ),
         True, expect="the evidence is gone, not merely moved")

    # The window, proven at its own edge and one line past it. A pair that
    # differs by a single line is the only way to show the window is neither
    # zero (which would red on any edit above) nor unbounded (which would make
    # the line number decorative and the check a plain `contains`).
    case("file anchor: content at the far EDGE of the window, still green",
         anchor("file:src/t.rs:%d:const ANCHORED_ONCE" % (_UNIQ - ANCHOR_WINDOW)),
         False)
    case("file anchor: content ONE line past the window has drifted",
         anchor("file:src/t.rs:%d:const ANCHORED_ONCE"
                % (_UNIQ - ANCHOR_WINDOW - 1)), True,
         expect="has MOVED -- cited at line")

    case("file anchor: a fragment matching two lines pins neither",
         anchor("file:src/t.rs:%d:const ANCHORED_TWICE" % _TWICE), True,
         expect="A fragment that matches more than once pins nothing")
    case("file anchor: a fragment too short to pin anything",
         anchor("file:src/t.rs:%d:;" % _UNIQ), True,
         expect="too short to pin anything")

    # THE defect #1198 was filed for. A line number with no content was the
    # accepted form until now, and it could not fail: this arm is the proof
    # that it does.
    case("BARE line anchor -- the wayland#1198 defect itself",
         anchor("file:src/t.rs:%d" % _UNIQ), True,
         expect="is a BARE line anchor")
    case("bare `file:<path>` with no line at all is still fine",
         anchor("file:src/t.rs"), False)
    case("met criterion with no evidence at all",
         lambda b: b.replace('    evidence: "test:src/t.rs::the_boundary_is_probed"\n',
                             ""), True, expect="is `met` with no evidence")
    case("symbol evidence naming an item that is not declared",
         lambda b: b.replace('"test:src/t.rs::the_boundary_is_probed"',
                             '"symbol:src/t.rs::AStructNobodyWrote"'), True,
         expect="declares no `AStructNobodyWrote`")
    case("symbol evidence naming an item that IS declared",
         lambda b: b.replace('"test:src/t.rs::the_boundary_is_probed"',
                             '"symbol:src/t.rs::Boundary"'), False)
    case("absent evidence for text that really is not in the file",
         lambda b: b.replace('"test:src/t.rs::the_boundary_is_probed"',
                             '"absent:src/t.rs::a_line_nobody_ever_wrote"'), False)
    case("absent evidence for text that is STILL THERE (wayland#1182 c3)",
         lambda b: b.replace('"test:src/t.rs::the_boundary_is_probed"',
                             '"absent:src/t.rs::pub struct Boundary"'), True,
         expect="still contains 'pub struct Boundary'")
    case("absent evidence over a path that is not here at all",
         lambda b: b.replace('"test:src/t.rs::the_boundary_is_probed"',
                             '"absent:src/gone.rs::anything"'), True,
         expect="an absence cannot be verified against a file that is not here")
    case("evidence that is prose rather than a token",
         lambda b: b.replace('"test:src/t.rs::the_boundary_is_probed"',
                             '"see the PR, it is obviously done"'), True,
         expect="is not a machine-resolvable evidence token")
    case("blocked criterion owned by core",
         lambda b: b.replace("    state: blocked\n    owner: maintainer",
                             "    state: blocked\n    owner: core"), True,
         expect="Core cannot block on itself")
    case("blocked criterion with no stated reason",
         lambda b: b.replace(
             '    note: "needs a Slack workspace credential the core lane does not hold"\n',
             ""), True, expect="is a suppression")
    case("frontmatter never closed",
         lambda b: b.replace("---\n\nProse", "\nProse"), True,
         expect="frontmatter is never closed")
    case("a value that opens a YAML structure the schema forbids",
         lambda b: b.replace("    state: met", "    state: [met, not-met]"), True,
         expect="starts a YAML structure this schema does not allow")
    case("last_verified_commit is not a commit in this tree",
         lambda b: b.replace("last_verified_commit: %s",
                             "last_verified_commit: deadbee"), True,
         expect="is not a commit in this tree")
    case("every criterion met while the issue is still open",
         lambda b: b.replace("state: not-met", "state: met").replace(
             "state: blocked", "state: met").replace(
             '  - id: c2\n    text: "the second half is not built yet"\n'
             '    state: met\n    owner: core\n', '') .replace(
             '  - id: c3\n    text: "the credentialled probe cannot run here"\n'
             '    state: met\n    owner: maintainer\n'
             '    note: "needs a Slack workspace credential the core lane does not hold"\n',
             ''), True, expect="marks every criterion met, but")
    case("superseded with no successor named in its note",
         lambda b: b.replace("    state: not-met\n    owner: core",
                             "    state: superseded\n    owner: core\n"
                             "    note: \"the rest of this moved somewhere, trust me\""),
         True, expect="names no successor issue")
    case("superseded into an issue that is CLOSED",
         lambda b: b.replace("    state: not-met\n    owner: core",
                             "    state: superseded\n    owner: core\n"
                             "    note: \"the residual is carried by #11 on the core tracker\""),
         True,
         inj={"all": {"FerroxLabs/wayland": {7: "open", 11: "closed"},
                      "FerroxLabs/wayland-core": {9: "open"}},
              "scoped": {"FerroxLabs/wayland": [7], "FerroxLabs/wayland-core": [9]}},
         expect="which is CLOSED")
    case("superseded into an issue that is OPEN",
         lambda b: b.replace("    state: not-met\n    owner: core",
                             "    state: superseded\n    owner: core\n"
                             "    note: \"the residual is carried by #11 on the core tracker\""),
         False,
         inj={"all": {"FerroxLabs/wayland": {7: "open", 11: "open"},
                      "FerroxLabs/wayland-core": {9: "open"}},
              "scoped": {"FerroxLabs/wayland": [7], "FerroxLabs/wayland-core": [9]}})
    case("an unmet criterion on an issue GitHub says is CLOSED",
         _ident, True,
         inj={"all": {"FerroxLabs/wayland": {7: "closed"},
                      "FerroxLabs/wayland-core": {9: "open"}},
              "scoped": {"FerroxLabs/wayland": [], "FerroxLabs/wayland-core": [9]}},
         expect="carries an unmet criterion, but")
    case("a ledger file for an issue that does not exist",
         _ident, True,
         inj={"all": {"FerroxLabs/wayland": {}, "FerroxLabs/wayland-core": {9: "open"}},
              "scoped": {"FerroxLabs/wayland": [], "FerroxLabs/wayland-core": [9]}},
         expect="which does not exist in that tracker")

    ok = True
    results = []
    for label, mutate, must_fire, offline, inj, expect in cases:
        body = mutate(_CLEAN)
        # Four arms deliberately leave the file alone -- two controls and two
        # that vary the TRACKER state instead -- and say so by passing _ident.
        # For every other arm, RED OR GREEN, an unchanged body means the
        # mutation stopped applying and the arm proves nothing. Restricting
        # this to red arms (as it did until #1198) leaves every green arm able
        # to pass as a second copy of the clean control, which is the same
        # vacuity one rung down.
        if mutate is not _ident and body == _CLEAN:
            print("  %-56s MUTATION DID NOT APPLY -- the arm tests nothing"
                  % label[:56])
            ok = False
            continue
        with tempfile.TemporaryDirectory() as td:
            _fixture(td, body, extra={"wayland-core-9.md": _CORE_9})
            code, out = run(td, offline=offline, injected=inj)
        fired = code != 0
        good = fired == must_fire
        if good and expect and expect not in "\n".join(out):
            good = False
            print("  %-56s fired, but not for its own reason (%r absent)"
                  % (label[:56], expect))
        ok &= good
        results.append((label, must_fire, fired, good))

    # THE check this whole file exists for, proven on its own: a whole tracker
    # missing. Same fixture, minus the wayland-core ledger file.
    with tempfile.TemporaryDirectory() as td:
        _fixture(td, _CLEAN)
        code, _ = run(td, injected=_INJ_CLEAN)
    fired = code != 0
    ok &= fired
    results.append(("AN ENTIRE TRACKER WITH NO LEDGER FILES", True, fired, fired))

    # And in the other direction, twice over: scanning nothing must never pass.
    for label, setup in (
        ("no ledger directory at all", lambda td: None),
        ("a ledger directory holding no files",
         lambda td: os.makedirs(os.path.join(td, LEDGER_DIR))),
    ):
        with tempfile.TemporaryDirectory() as td:
            setup(td)
            code, _ = run(td, offline=True)
        fired = code != 0
        ok &= fired
        results.append((label, True, fired, fired))

    # A tracker query that reaches nothing is a broken query, not a clean repo.
    with tempfile.TemporaryDirectory() as td:
        _fixture(td, _CLEAN, extra={"wayland-core-9.md": _CORE_9})
        code, _ = run(td, injected={
            "all": {"FerroxLabs/wayland": {7: "open"},
                    "FerroxLabs/wayland-core": {9: "open"}},
            "scoped": {"FerroxLabs/wayland": [7], "FerroxLabs/wayland-core": [9]}})
    results.append(("control after the vacuity arms (still green)",
                    False, code != 0, code == 0))
    ok &= code == 0

    for label, must, got, good in results:
        print("  %-56s expected %-5s got %-5s  %s"
              % (label[:56], "RED" if must else "green", "RED" if got else "green",
                 "ok" if good else "SELF-TEST FAILED"))
    print("self-test: %s"
          % ("both directions proven" if ok else "BROKEN -- the gate cannot be trusted"))
    return 0 if ok else 1


def main(argv):
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    # There is deliberately NO flag to inject tracker state from the command
    # line. `run(injected=...)` exists for the self-test's fixtures and is
    # reachable only from inside this file; a CLI switch that hands the gate a
    # tracker snapshot is a switch that makes the coverage check say whatever
    # the caller wants, and the only place it would ever be typed is a CI
    # workflow trying to get green.
    code, out = run(root, offline="--offline" in argv)
    print("\n".join(out))
    return code


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(main(sys.argv))
