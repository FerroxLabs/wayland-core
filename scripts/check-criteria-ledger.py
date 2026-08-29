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
        file:<path>:<line>         file exists AND has at least <line> lines
        file:<path>                file exists
        commit:<sha>               resolves to a commit object

    Prefer `test:` and `symbol:` over `file:<path>:<line>`. A line number is
    the weakest anchor here: it survives an edit that moves the code it names,
    so it can go on pointing at nothing in particular while staying green.
    `file:` with a line is for evidence that is genuinely positional -- a
    workflow step, a table row -- and the prose should say what is there.
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
FILE_EV = re.compile(r"^file:(?P<p>.+?)(?::(?P<l>\d+))?$")
COMMIT_EV = re.compile(r"^commit:(?P<s>[0-9a-f]{7,40})$")
SLUG = re.compile(r"^(?P<slug>[a-z0-9][a-z0-9-]*?)-(?P<num>\d+)\.md$")


def _is_git(root):
    return subprocess.run(
        ["git", "-C", root, "rev-parse", "--git-dir"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    ).returncode == 0


def resolve_evidence(root, ev, git):
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
    m = FILE_EV.match(ev)
    if m:
        p = os.path.join(root, m.group("p"))
        if not os.path.isfile(p):
            return "no such file: %s" % m.group("p")
        if m.group("l"):
            n = sum(1 for _ in open(p, encoding="utf-8", errors="replace"))
            if int(m.group("l")) > n:
                return "%s has %d lines; evidence cites line %s" % (
                    m.group("p"), n, m.group("l"))
        return None
    m = COMMIT_EV.match(ev)
    if m:
        if not git:
            return "commit evidence needs a git worktree; this root is not one"
        r = subprocess.run(
            ["git", "-C", root, "cat-file", "-t", m.group("s")],
            capture_output=True, text=True)
        if r.returncode != 0 or r.stdout.strip() != "commit":
            return "%s does not resolve to a commit in this tree" % m.group("s")
        return None
    return ("%r is not a machine-resolvable evidence token. Use "
            "test:<path>::<name>, symbol:<path>::<name>, file:<path>[:<line>] "
            "or commit:<sha>." % ev)


def validate_record(root, rec, git):
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
    elif git:
        r = subprocess.run(["git", "-C", root, "cat-file", "-t", sha],
                           capture_output=True, text=True)
        if r.stdout.strip() != "commit":
            bad.append("%s: last_verified_commit %s is not a commit in this "
                       "tree -- the entry was verified against something that "
                       "is not here" % (p, sha))
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
            why = resolve_evidence(root, ev, git)
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
        problems += validate_record(root, rec, git)

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


def _fixture(root, body, name="wayland-7.md", extra=None):
    d = os.path.join(root, LEDGER_DIR)
    os.makedirs(d, exist_ok=True)
    os.makedirs(os.path.join(root, "src"), exist_ok=True)
    open(os.path.join(root, "src", "t.rs"), "w").write(
        "pub struct Boundary;\n"
        "#[test]\nfn the_boundary_is_probed() { assert!(true); }\n")
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


def self_test():
    cases = []

    def case(label, mutate, must_fire, offline=False, inj=_INJ_CLEAN, expect=None):
        # `expect` is not decoration. A red arm that fires for the WRONG reason
        # proves nothing about the check it was written for, and a mutation
        # that silently stops applying (one did, on the first run of this
        # file) reads as a passing gate. Every RED arm names its own message.
        cases.append((label, mutate, must_fire, offline, inj, expect))

    case("clean control, both trackers covered", lambda b: b, False)
    case("control again, offline", lambda b: b, False, offline=True)
    case("met criterion cites a test that does not exist",
         lambda b: b.replace("::the_boundary_is_probed\"",
                             "::a_test_that_was_deleted\"", 1), True,
         expect="declares no `fn a_test_that_was_deleted`")
    case("met criterion cites a file line past EOF",
         lambda b: b.replace('"test:src/t.rs::the_boundary_is_probed"',
                             '"file:src/t.rs:9000"'), True,
         expect="evidence cites line 9000")
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
         lambda b: b, True,
         inj={"all": {"FerroxLabs/wayland": {7: "closed"},
                      "FerroxLabs/wayland-core": {9: "open"}},
              "scoped": {"FerroxLabs/wayland": [], "FerroxLabs/wayland-core": [9]}},
         expect="carries an unmet criterion, but")
    case("a ledger file for an issue that does not exist",
         lambda b: b, True,
         inj={"all": {"FerroxLabs/wayland": {}, "FerroxLabs/wayland-core": {9: "open"}},
              "scoped": {"FerroxLabs/wayland": [], "FerroxLabs/wayland-core": [9]}},
         expect="which does not exist in that tracker")

    ok = True
    results = []
    for label, mutate, must_fire, offline, inj, expect in cases:
        body = mutate(_CLEAN)
        # Two arms deliberately leave the file alone and vary the TRACKER
        # state instead; for every other red arm an unchanged body means the
        # mutation stopped applying and the arm proves nothing.
        if must_fire and body == _CLEAN and inj is _INJ_CLEAN:
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
