#!/usr/bin/env python3
"""Refuse to cut a release while an in-scope DEFECT still owes core work.

WHY THIS EXISTS
    `scripts/check-criteria-ledger.py` gates the BOOKKEEPING. It fails on a
    malformed entry, on a `met` with no evidence, on evidence that no longer
    resolves, on a `blocked` owned by core, and on an open issue with no
    ledger file. Every one of those is a check that the RECORD is honest.

    Nothing checks that the WORK is done. On the tree this file was written
    against, 67 criteria are `not-met` and owned by `core`, and
    `just ledger-check` is completely green. It has never been possible for
    this repo to go red because a release was incomplete -- only because a
    ledger lied about it. That is the mechanism behind every partial release
    here: v0.13.10 shipped claiming 22 issues closed, and grading them
    against their own stated criteria found 9.

    The rule this file enforces is the maintainer's: a ticket ends CLOSED or
    DECOMPOSED -- core's half closed, and the remainder filed as its own
    ticket against a named owner with the contract spelled out. "Partial" is
    not a status; it is a ticket somebody failed to split.

WHAT IS IN SCOPE, AND HOW IT IS DECIDED
    Errors, problems and issues block a release. Feature requests do not.
    An unshipped feature is a roadmap item; an unfixed defect is something a
    user is living with in the version we are about to publish.

    The tracker labels that would say which is which (`bug`, `enhancement`)
    live on GitHub, and the offline arm must work with no network -- so the
    classification is a first-class field in the ledger file itself:

        kind: defect      # or: feature, task

    `task` = every remaining criterion is a credential, an account or a
    platform a human must obtain; there is no code behind it. Excluded from
    blocking for the same reason `feature` is, and held to the same
    corroboration rule: a `task` the tracker labels `bug` is a hard failure.

    It is REQUIRED. A missing `kind` is a hard failure, not a default,
    because a default is a bypass: whichever way it defaulted, the next
    entry written would silently land in the convenient bucket and nobody
    would ever type the word. The live arm corroborates the field against
    the tracker's own labels, and fails when GitHub calls something a `bug`
    that the ledger has classified out of scope -- which is the one
    direction of misclassification that shrinks the blocking set.

    Where a title and body were genuinely ambiguous, the entry was written
    `defect`. Over-blocking costs a conversation; under-blocking ships the
    bug.

WHAT IT FAILS ON
    * a ledger entry with no `kind:`, or a `kind:` that is not
      `defect` / `feature` / `task`                           (unclassified)
    * `kind: defect` with a criterion `state: not-met, owner: core`
      -- core still owes work on a defect                       (OUTSTANDING)
    * `kind: defect` with a criterion owned by desktop/flux/maintainer/
      reporter that is `blocked` or `not-met` and carries no `handoff:`
      naming the ticket that now owns it                     (UNDECOMPOSED)
      A remainder nobody can find is a partial wearing a label. The existing
      ledger gate already refuses a `blocked` owned by `core`; this is the
      other half of the same rule, applied to the lanes core hands work TO.
      `not-met` is included alongside `blocked` deliberately: they are the
      same invisibility with a different word, and if only one of them
      needed a `handoff` the escape would be to type the other one.
    * a `handoff:` that is not `<owner>/<repo>#<number>`. A bare `#1187` is
      ambiguous across two trackers, and this project has already lost a
      release to a second tracker nobody could see.
    * (live) a `handoff:` target that does not exist, or that is CLOSED.
      A residual handed to a closed issue is untracked with extra steps.
    * (live) an entry marked `kind: feature` that its tracker labels `bug`.
    * scanning nothing -- no ledger directory, no files, no criteria, or a
      tree in which not one entry is a `defect`. A gate that examined sixty
      files and had no defect to judge did not pass; it abstained.
    * a workspace version it cannot read, or that does not reduce to an
      X.Y.Z milestone. The release this gate judges is DERIVED from
      `[workspace.package] version` in the root Cargo.toml -- it has no
      default, because a gate that defaults its own scope grades whichever
      release is convenient rather than the one being cut.

WHAT IT DOES NOT DO
    It does not judge whether a criterion is a GOOD criterion, whether a
    `met` is true, or whether the evidence resolves. That is
    check-criteria-ledger.py's job and it is not duplicated here -- this
    file imports that one's parser precisely so the two gates can never
    disagree about what a ledger file says.

    It is NOT in `check-all`. See the justfile comment: a gate that is red
    on every in-progress lane gets bypassed, and then ignored, and then it
    is not a ratchet. This one is red by design until the work is done, so
    it belongs on the release path and nowhere else.

    python3 scripts/check-release-readiness.py --self-test  # prove both ways
    python3 scripts/check-release-readiness.py              # the gate (needs gh)
    python3 scripts/check-release-readiness.py --offline    # structure only
"""
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))

KINDS = ("defect", "feature", "task")
# Owners that are NOT core. A criterion in one of these hands work out of the
# lane, and handing work out is exactly the moment a remainder goes invisible.
FOREIGN_OWNERS = ("desktop", "flux", "maintainer", "reporter")
# States that mean the work is outstanding. `superseded` is absent on purpose:
# the ledger gate already refuses a `superseded` whose note names no successor
# issue, and verifies that successor exists and is open. That IS decomposition,
# so re-blocking on it here would be this gate second-guessing a check that
# already passed.
OPEN_STATES = ("not-met", "blocked")

# The release this gate judges. DERIVED from the tree, not typed into it.
#
# It was a constant, and the constant did what constants do: it outlived its
# release. This file shipped reading `0.13.12` AFTER 0.13.12 was published, so
# the gate was grading a milestone whose work is finished by definition. It
# passed trivially and certified nothing about the release actually being cut.
# That is the same failure class the rest of this file exists to catch -- a
# check that cannot go red -- sitting in the check's own scope.
#
# The number it reads is `[workspace.package] version`, which is already the
# tree's statement of which release it is: release.yml's "Extract version from
# tag" step refuses to tag when that value and the tag disagree. Deriving from
# it means the gate and the tag can never grade different releases.
#
# It is still not a flag, for the same reason the `injected` fixture below has
# none: a switch that changes which issues count is a switch that gets typed
# when a workflow needs green. The only way to move this one is to bump the
# version the release is genuinely cut from, in Cargo.toml, in the open.
# Leftovers become the next release's scope by being re-milestoned on the
# tracker, one issue at a time -- never by a default here.
#
# An OPEN issue carrying no milestone is a hard failure below. Without that this
# would be a bypass and not a scope: an unmilestoned issue would sit outside
# every release forever and no gate would ever say so.


class MilestoneError(Exception):
    pass


# The SAME section-scoped read release.yml performs, in Python:
#
#     sed -n '/^\[workspace\.package\]/,/^\[/ s/^version *= *"\(.*\)"/\1/p' \
#         Cargo.toml | head -1
#
# Section-scoped on purpose. `version = ` appears in other tables of this
# manifest, and a whole-file grep would grade a release that does not exist.
_WS_VERSION = re.compile(r'^version\s*=\s*"([^"]*)"')
_SEMVER_BASE = re.compile(r"^\d+\.\d+\.\d+$")


def workspace_version(root):
    """-> the `[workspace.package] version` string from <root>/Cargo.toml.

    Raises MilestoneError on every failure. It never returns a fallback: the
    caller must not be able to keep going with a guessed scope.
    """
    path = os.path.join(root, "Cargo.toml")
    try:
        with open(path, encoding="utf-8") as fh:
            text = fh.read()
    except OSError as e:
        raise MilestoneError(
            "could not read %s (%s). This gate derives the release it judges "
            "from the workspace version and has no default to fall back on."
            % (path, e))
    in_table = False
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("["):
            if in_table:
                break  # the sed range closes at the next table header
            in_table = (s == "[workspace.package]")
            continue
        if in_table:
            m = _WS_VERSION.match(s)
            if m:
                return m.group(1)
    raise MilestoneError(
        'no `version = "..."` under `[workspace.package]` in %s. That key is '
        "what release.yml compares a tag against; without it there is no "
        "release for this gate to grade." % path)


def release_milestone(root):
    """-> the tracker milestone title for the release <root> is cut from.

    A pre-release tree grades the milestone it is a candidate FOR: a tree at
    `0.13.13-rc.2` is being cut toward `0.13.13`, the tracker has no
    `0.13.13-rc.2` milestone and never will, and release.yml already requires
    an rc tag to share its BASE version with the tree (`${version%%-*}`). The
    strip here is that same rule, so the gate and the tag agree by
    construction.

    Anything that does not reduce to an X.Y.Z base is REFUSED, not guessed.
    """
    version = workspace_version(root)
    base = version.split("-", 1)[0]
    if not _SEMVER_BASE.match(base):
        raise MilestoneError(
            "workspace version %r in %s does not reduce to an X.Y.Z milestone "
            "(%r after stripping the pre-release suffix). Refusing to "
            "guess: a gate that guesses its own scope grades the wrong "
            "release."
            % (version, os.path.join(root, "Cargo.toml"), base))
    return base

HANDOFF = re.compile(r"^(?P<repo>[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)#(?P<num>\d+)$")
# Label names that decide the question on GitHub. Anything else is silence,
# and silence is not corroboration -- it is reported as such, never as a pass.
BUG_LABELS = {"bug", "defect", "type:bug", "kind/bug", "regression"}
FEATURE_LABELS = {"enhancement", "feature", "type:feature", "kind/feature"}


def load_ledger_module():
    """The sibling gate, imported for its parser. Hyphens, hence importlib.

    Sharing the parser is the point. Two independent readers of the same
    frontmatter is two grammars that drift, and the first symptom of the
    drift is one gate quietly not seeing an entry the other one does.
    """
    p = os.path.join(HERE, "check-criteria-ledger.py")
    if not os.path.isfile(p):
        raise RuntimeError(
            "%s is missing. This gate reads ledger files with that file's "
            "parser and refuses to grow a second one." % p)
    spec = importlib.util.spec_from_file_location("check_criteria_ledger", p)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


# ── tracker state ────────────────────────────────────────────────────────────


class TrackerError(Exception):
    pass


GH_LIMIT = 4000


def gh_labels(repo):
    """-> {number: set(label names)} for every issue in `repo`, any state."""
    args = ["gh", "issue", "list", "-R", repo, "--state", "all",
            "--limit", str(GH_LIMIT), "--json", "number,labels"]
    try:
        r = subprocess.run(args, capture_output=True, text=True)
    except OSError as e:
        raise TrackerError(
            "%s: could not run `gh` (%s). The corroboration arm needs it; "
            "pass --offline if you meant to skip it, and read what --offline "
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
            "%s: the query returned exactly %d rows, which is the request "
            "limit -- it was TRUNCATED. Raise GH_LIMIT." % (repo, GH_LIMIT))
    if not rows:
        raise TrackerError(
            "%s: reached ZERO issues in any state. That is not a clean "
            "tracker, it is a broken query." % repo)
    return {int(i["number"]): {l["name"] for l in i["labels"]} for i in rows}


def gh_milestones(repo):
    """-> {number: milestone title or None} for every OPEN issue in `repo`.

    Open only, deliberately. A closed issue's milestone cannot block anything,
    and asking for every state would fail the gate on historical issues nobody
    is ever going to re-milestone.
    """
    args = ["gh", "issue", "list", "-R", repo, "--state", "open",
            "--limit", str(GH_LIMIT), "--json", "number,milestone"]
    try:
        r = subprocess.run(args, capture_output=True, text=True)
    except OSError as e:
        raise TrackerError(
            "%s: could not run `gh` (%s). The milestone arm decides what is in "
            "scope for this release; pass --offline if you meant to skip it."
            % (repo, e))
    if r.returncode != 0:
        raise TrackerError("%s: `gh issue list` (milestones) failed -- %s"
                           % (repo, (r.stderr or "").strip()[:300]))
    try:
        rows = json.loads(r.stdout)
    except ValueError as e:
        raise TrackerError("%s: unparseable gh milestone output (%s)" % (repo, e))
    if len(rows) >= GH_LIMIT:
        raise TrackerError(
            "%s: the milestone query returned exactly %d rows, which is the "
            "request limit -- it was TRUNCATED. Raise GH_LIMIT." % (repo, GH_LIMIT))
    out = {}
    for i in rows:
        m = i.get("milestone") or {}
        out[int(i["number"])] = m.get("title") or None
    return out


def gh_issue_state(ref):
    """-> 'open' / 'closed' / None for `<owner>/<repo>#<number>`."""
    repo, _, num = ref.partition("#")
    r = subprocess.run(
        ["gh", "issue", "view", num, "-R", repo, "--json", "state"],
        capture_output=True, text=True)
    if r.returncode != 0:
        return None
    try:
        return json.loads(r.stdout)["state"].lower()
    except (ValueError, KeyError):
        return None


def fetch_tracker_state(repos, handoffs, injected, milestone):
    """-> ({repo: {num: labels}}, {handoff ref: state-or-None}).

    `injected` short-circuits the network for --self-test's fixtures. It has
    no command-line switch, for the same reason the sibling gate's has none:
    a flag that hands the gate its own tracker snapshot is a flag that makes
    the gate say whatever the caller needs it to say, and the only place it
    would ever be typed is a workflow trying to get green.
    """
    if injected is not None:
        labels = {}
        for ref, names in injected.get("labels", {}).items():
            repo, _, num = ref.partition("#")
            labels.setdefault(repo, {})[int(num)] = set(names)
        for repo in repos:
            labels.setdefault(repo, {})
        # Fixture default: an injected issue with no explicit milestone is
        # treated as being in THIS release. Without it every arm below is
        # filtered out of the blocking set before it can be observed, which is
        # exactly what the self-test caught when this filter was first written.
        # The default exists ONLY here. The live path queries the tracker and
        # has no default at all; that asymmetry is what keeps the milestone a
        # scope and not a bypass.
        miles = {}
        for rp, nums in labels.items():
            for num in nums:
                miles.setdefault(rp, {})[num] = milestone
        for ref, title in injected.get("milestones", {}).items():
            rp, _, num = ref.partition("#")
            miles.setdefault(rp, {})[int(num)] = title
        for repo in repos:
            miles.setdefault(repo, {})
        return (labels, miles,
                {h: injected.get("issues", {}).get(h) for h in handoffs})
    labels = {repo: gh_labels(repo) for repo in sorted(repos)}
    miles = {repo: gh_milestones(repo) for repo in sorted(repos)}
    return labels, miles, {h: gh_issue_state(h) for h in sorted(handoffs)}


# ── the gate ─────────────────────────────────────────────────────────────────


def classify(rec, problems):
    """-> 'defect' / 'feature' / None, appending a complaint when unusable."""
    kind = (rec.get("kind") or "").strip()
    if not kind:
        problems.append(
            "%s: no `kind:` field. Every ledger entry must say `kind: defect`, "
            "`kind: feature` or `kind: task` -- defects block a release, the "
            "other two do not, and a field that defaults is a field nobody "
            "ever types."
            % rec["path"])
        return None
    if kind not in KINDS:
        problems.append("%s: kind %r must be one of %s"
                        % (rec["path"], kind, "/".join(KINDS)))
        return None
    return kind


def run(root, offline=False, injected=None):
    """-> (exit code, [lines])."""
    out = []

    def say(s=""):
        out.append(s)

    # First, because everything below is scoped by it. A tree that cannot say
    # which release it is cannot be graded against one, and the only honest
    # answer at that point is a non-zero exit -- never a default milestone.
    try:
        milestone = release_milestone(root)
    except MilestoneError as e:
        say("FAIL: %s" % e)
        return 2, out

    try:
        L = load_ledger_module()
    except RuntimeError as e:
        say("FAIL: %s" % e)
        return 2, out

    files = L.collect(root)
    if files is None:
        say("FAIL: %s does not exist. There is no ledger to grade, which is "
            "not the same as a release with nothing outstanding." % L.LEDGER_DIR)
        return 2, out
    if not files:
        say("FAIL: %s holds no ledger files. A gate that scans nothing cannot "
            "fail, and this repo has shipped one of those before." % L.LEDGER_DIR)
        return 2, out

    records, problems = [], []
    for f in files:
        rec, errs = L.parse_ledger(f)
        # A file this gate cannot read is a file it cannot clear. It does not
        # report the detail -- that is the ledger gate's output, and two gates
        # printing the same parse errors trains people to read neither.
        if errs:
            problems.append(
                "%s: does not parse. `just ledger-check` owns the detail; this "
                "gate cannot grade an entry it cannot read." % f)
        if rec is not None:
            records.append(rec)

    ncrit = sum(len(r["criteria"]) for r in records)
    if ncrit == 0:
        say("FAIL: %d ledger file(s) and zero criteria between them." % len(files))
        return 2, out

    kinds = {}
    for rec in records:
        kinds[rec["path"]] = classify(rec, problems)

    ndefect = sum(1 for k in kinds.values() if k == "defect")
    nfeature = sum(1 for k in kinds.values() if k == "feature")
    ntask = sum(1 for k in kinds.values() if k == "task")
    say("ledger files: %d   criteria: %d   defect %d / feature %d / task %d / "
        "unclassified %d"
        % (len(files), ncrit, ndefect, nfeature, ntask,
           len(kinds) - ndefect - nfeature - ntask))

    if ndefect == 0 and not problems:
        # Sixty files, none of them a defect, and therefore nothing this gate
        # could ever have refused. That is not a clean tree; it is an
        # abstention, and it is the shape a bypass would take.
        say()
        say("FAIL: not one ledger entry is `kind: defect`, so this gate had "
            "nothing in scope to judge. A tree in which no open issue is a "
            "problem is a misclassification, not a clean release.")
        return 2, out

    # ── the blocking set ────────────────────────────────────────────────
    outstanding = []   # (rec, crit) -- core still owes work on a defect
    undecomposed = []  # (rec, crit) -- handed out, with nothing carrying it
    handoffs = set()

    for rec in records:
        # Only `defect` blocks. `feature` is an unshipped roadmap item; `task`
        # is a ticket whose every remaining criterion is an account, a token or
        # a platform a human must obtain, with no code change behind it. Neither
        # is something a user is living with in what we are about to publish.
        #
        # `task` exists because without it this gate could never go green. A
        # credentials shopping list that nobody may ever be able to buy would
        # block every release forever, and a gate that cannot PASS is worth as
        # little as one that cannot fail. It is NOT an escape hatch: a `task`
        # that GitHub labels `bug` is a hard failure below, exactly as a
        # `feature` is, and the DEFECT a task was split out of keeps blocking on
        # its own row -- wayland#1186 is the credentials list, wayland#934 is the
        # defect, and #934 does not stop blocking because #1186 was filed.
        if kinds.get(rec["path"]) != "defect":
            continue
        for c in rec["criteria"]:
            state, owner = c.get("state"), c.get("owner")
            ho = (c.get("handoff") or "").strip()
            if ho:
                if not HANDOFF.match(ho):
                    problems.append(
                        "%s:%s: handoff %r is not `<owner>/<repo>#<number>`. A "
                        "bare issue number is ambiguous across two trackers, "
                        "and an entire tracker going unseen is what this "
                        "project already lost a release to."
                        % (rec["path"], c.get("id"), ho))
                else:
                    handoffs.add(ho)
            if state not in OPEN_STATES:
                continue
            if owner == "core":
                if state == "not-met":
                    outstanding.append((rec, c))
                # `blocked` owned by core is already a hard failure in
                # check-criteria-ledger.py. Repeating it here would double-count
                # one defect across two gates and teach nobody anything.
                continue
            if owner in FOREIGN_OWNERS and not ho:
                undecomposed.append((rec, c))

    # ── live corroboration ──────────────────────────────────────────────
    # None means the tracker was never consulted, so release scope is NOT
    # judged below. That is not a pass; the offline banner says so.
    miles = None
    if offline:
        say()
        say("OFFLINE: handoff targets were NOT resolved, and `kind:` was NOT "
            "cross-checked against the trackers' own labels. THIS IS NOT A "
            "PASS for either. A `handoff:` naming an issue that is closed or "
            "does not exist reads exactly like a real one here, and an entry "
            "marked `kind: feature` that GitHub labels `bug` has removed "
            "itself from the blocking set with nothing contradicting it. "
            "Re-run without --offline before cutting anything.")
    else:
        repos = sorted({r.get("repo") for r in records if r.get("repo")})
        try:
            labels, miles, ho_state = fetch_tracker_state(
                repos, handoffs, injected, milestone)
        except TrackerError as e:
            say()
            say("FAIL: %s" % e)
            say("The gate refuses to degrade to a structural check silently. "
                "Pass --offline if you meant to skip corroboration.")
            return 2, out
        if injected is not None:
            say("NOTE: tracker state was INJECTED. This is a fixture run, not "
                "a gate run.")

        uncorroborated = []
        for rec in records:
            kind = kinds.get(rec["path"])
            if kind is None:
                continue
            repo, num = rec.get("repo"), rec.get("issue")
            got = labels.get(repo, {}).get(int(num)) if str(num).isdigit() else None
            if got is None:
                if kind in ("feature", "task"):
                    uncorroborated.append("%s#%s (not found on the tracker)"
                                          % (repo, num))
                continue
            if kind in ("feature", "task") and (got & BUG_LABELS):
                problems.append(
                    "MISCLASSIFIED: %s says `kind: %s`, but %s#%s is "
                    "labelled %s on GitHub. A defect filed out of scope is the "
                    "one direction of misclassification that shrinks this "
                    "gate's blocking set."
                    % (rec["path"], kind, repo, num,
                       "/".join(sorted(got & BUG_LABELS))))
            elif kind == "task":
                # A `task` is never corroborated by a label -- no tracker has a
                # "this is a shopping list" label -- so every one is a judgement
                # call and every one shrinks the blocking set. Name them all.
                uncorroborated.append("%s#%s [kind: task] (labels: %s)"
                                      % (repo, num,
                                         ", ".join(sorted(got)) or "none"))
            elif kind == "feature" and not (got & FEATURE_LABELS):
                uncorroborated.append("%s#%s (labels: %s)"
                                      % (repo, num,
                                         ", ".join(sorted(got)) or "none"))
            elif kind == "defect" and (got & FEATURE_LABELS) and not (got & BUG_LABELS):
                # Reported, NOT failed. This direction over-blocks, and an
                # over-block costs a conversation while an under-block ships
                # the bug. But it is printed, because a `defect` nobody agrees
                # with is a row somebody should look at.
                say("NOTE: %s says `kind: defect` while %s#%s is labelled %s. "
                    "Blocking anyway -- over-blocking is the safe direction -- "
                    "but the classification is worth a second look."
                    % (rec["path"], repo, num,
                       "/".join(sorted(got & FEATURE_LABELS))))

        def _ledger_for(recs, target):
            """Criterion ids the carrier still owes, [] if none, None if unledgered."""
            for r in recs:
                if "%s#%s" % (r.get("repo"), r.get("issue")) != target:
                    continue
                return [c.get("id") for c in r.get("criteria", [])
                        if c.get("state") in ("not-met", "blocked")]
            return None

        discharged = []
        for ho in sorted(handoffs):
            st = ho_state.get(ho)
            if st is None:
                problems.append(
                    "HANDOFF: %s does not exist. A remainder handed to an "
                    "issue nobody can open is not decomposed, it is deleted."
                    % ho)
            elif st == "closed":
                # A carrier closed BECAUSE it is finished is not a hole. Ask the
                # carrier's own ledger, not just the tracker: still owing work
                # (any not-met or blocked criterion) means the residual really
                # did go nowhere; owing nothing means the decomposition
                # completed. No ledger at all still fails -- an untracked
                # carrier is precisely what this rule exists to catch.
                carrier = _ledger_for(records, ho)
                if carrier is None:
                    problems.append(
                        "HANDOFF: %s is CLOSED and has no ledger, so nothing "
                        "records whether the residual was finished or dropped."
                        % ho)
                elif carrier:
                    problems.append(
                        "HANDOFF: %s is CLOSED while its own ledger still owes "
                        "work (%s). A residual carried by a closed issue that "
                        "is not finished is untracked with extra steps."
                        % (ho, ", ".join(carrier)))
                else:
                    discharged.append(ho)

        say("corroborated `kind:` against tracker labels for %d issue(s) "
            "across %s" % (sum(len(v) for v in labels.values()),
                           ", ".join(repos) or "no trackers"))
        say("resolved %d handoff target(s)" % len(handoffs))
        if discharged:
            say("%d handoff target(s) are CLOSED and owe nothing on their own "
                "ledger -- the residual completed rather than went missing: %s"
                % (len(discharged), ", ".join(sorted(discharged))))
        if uncorroborated:
            # Not a failure: most of the second tracker carries no labels at
            # all. But a `feature` is the classification that removes work
            # from the blocking set, so every one nothing corroborates is
            # named here rather than absorbed into a green line.
            say("NOTE: %d `kind: feature`/`kind: task` classification(s) that no "
                "tracker label corroborates -- each of these is a judgement call, and "
                "each one shrinks the blocking set:" % len(uncorroborated))
            for u in uncorroborated:
                say("        " + u)

    # ── release scope ───────────────────────────────────────────
    # Only what is milestoned for THIS release blocks it. Everything else
    # keeps its ledger, its criteria and its owner -- tracked, not dropped
    # -- but does not stand between a user and a fix that is ready.
    deferred, unmilestoned = {}, []
    if miles is not None:
        def _ms(rec):
            num = rec.get("issue")
            if not str(num).isdigit():
                return None
            return miles.get(rec.get("repo"), {}).get(int(num))

        keep_o, keep_u = [], []
        for bucket, keep in ((outstanding, keep_o), (undecomposed, keep_u)):
            for rec, c in bucket:
                ms = _ms(rec)
                if ms is None:
                    unmilestoned.append((rec.get("repo"), rec.get("issue")))
                    continue
                if ms != milestone:
                    deferred.setdefault(ms, set()).add(
                        "%s#%s" % (rec.get("repo"), rec.get("issue")))
                    continue
                keep.append((rec, c))
        outstanding, undecomposed = keep_o, keep_u

        for rp, num in sorted(set(unmilestoned)):
            problems.append(
                "%s#%s is an OPEN defect owing work and carries NO "
                "milestone, so no release claims it and none ever will. "
                "Put it in a milestone. An unmilestoned issue is not out of "
                "scope -- it is invisible TO scope, which is exactly what a "
                "defaulting `kind` would have been." % (rp, num))
        for ms in sorted(deferred):
            say("NOTE: %d issue(s) owe work under milestone `%s`, not `%s`. "
                "They are tracked and are NOT part of this release's "
                "definition of done: %s"
                % (len(deferred[ms]), ms, milestone,
                   ", ".join(sorted(deferred[ms]))))
        if deferred:
            say()

    # ── verdict ─────────────────────────────────────────────────────────
    if problems:
        say()
        say("FAIL: %d problem(s) with the ledger's release-readiness fields."
            % len(problems))
        for p in problems:
            say("  " + p)
        return 1, out

    if outstanding or undecomposed:
        say()
        by_issue = {}
        for rec, c in outstanding:
            by_issue.setdefault(rec["path"], (rec, [], []))[1].append(c)
        for rec, c in undecomposed:
            by_issue.setdefault(rec["path"], (rec, [], []))[2].append(c)
        say("RELEASE BLOCKED (%s): %d defect issue(s) still owe work -- %d "
            "core-owned criterion(s) not met, %d handed to another lane with "
            "nothing tracking the remainder."
            % (milestone, len(by_issue), len(outstanding),
               len(undecomposed)))
        say("This list IS the definition of done for the release. A ticket "
            "ends CLOSED or DECOMPOSED; `partial` is a ticket nobody split.")
        say()
        for path in sorted(by_issue):
            rec, outs, unds = by_issue[path]
            say("  %s#%s  %s" % (rec.get("repo"), rec.get("issue"),
                                 rec.get("title", "")))
            for c in outs:
                say("      OUTSTANDING   %-4s %s"
                    % (c.get("id"), c.get("text", "")))
            for c in unds:
                say("      UNDECOMPOSED  %-4s owner=%s state=%s -- no "
                    "`handoff:` names the ticket that now carries this"
                    % (c.get("id"), c.get("owner"), c.get("state")))
        return 1, out

    say()
    say("OK: every `kind: defect` entry has zero core-owned criteria "
        "outstanding, and every criterion handed to another lane names the "
        "ticket that carries it%s"
        % (" (handoff targets and labels not checked -- offline)." if offline
           else ", which exists and is open."))
    return 0, out


# ── self-test ────────────────────────────────────────────────────────────────
#
# The most important part of this file. A gate that cannot fail is worth
# exactly what a gate that cannot pass is worth, and this repo has shipped one
# of each. Every arm below states which direction it expects, and every RED arm
# names the message it must fire with -- an arm that goes red for somebody
# else's reason has proved nothing about the check it was written for.

_DEFECT = """---
issue: 7
repo: FerroxLabs/wayland
kind: defect
title: "a defect whose core half is finished"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "the boundary is probed rather than asserted against itself"
    state: met
    evidence: "test:src/t.rs::the_boundary_is_probed"
    owner: core
  - id: c2
    text: "the credentialled probe cannot run in this lane"
    state: blocked
    owner: maintainer
    handoff: "FerroxLabs/wayland#11"
    note: "needs a Slack workspace credential the core lane does not hold"
---

Prose a human with no context can read: this is the fixture control for the
release-readiness gate's own self-test, and it must stay green in every arm.
"""

_FEATURE = """---
issue: 8
repo: FerroxLabs/wayland
kind: feature
title: "a feature request, which does not block a release"
status: open
last_verified_commit: cfa89a9c
criteria:
  - id: c1
    text: "the opt-in flag exists"
    state: not-met
    owner: core
---

Prose a human with no context can read: an unshipped feature is a roadmap item,
not something a user is living with in the version about to be published.
"""

_INJ = {
    "labels": {"FerroxLabs/wayland#7": ["bug", "area:core"],
               "FerroxLabs/wayland#8": ["enhancement", "area:core"]},
    "issues": {"FerroxLabs/wayland#11": "open"},
}


# A sentinel, because `None` has to mean "write no such file" -- the arm that
# proves a tree with no defect in it goes red needs to omit the defect file
# entirely, and a default-argument `None` cannot say both things.
_KEEP = object()

# The fixture tree's own version, deliberately nothing this repo has ever been
# or will be. If derivation ever silently read the REAL Cargo.toml instead of
# the fixture's, every milestone arm below would grade `0.13.x` and the A/B
# would collapse into two identical runs.
_FIXTURE_VERSION = "9.9.9"
_FIXTURE_NEXT = "9.9.10"

# The decoys are the point. `version = ` appears in three tables here and only
# one of them is the release. A reader that greps the whole file, or that stops
# at the first hit, grades a version that does not exist.
_CARGO = """[workspace]
resolver = "2"
members = ["crates/x"]
version = "0.0.0-decoy-before"

[workspace.package]
version = "%s"
edition = "2024"

[workspace.dependencies]
version = "0.0.0-decoy-after"
"""


def _cargo(version):
    return _CARGO % version


def _fixture(root, defect, feature, cargo):
    d = os.path.join(root, ".planning", "ledger")
    os.makedirs(d, exist_ok=True)
    if defect is not None:
        open(os.path.join(d, "wayland-7.md"), "w").write(defect)
    if feature is not None:
        open(os.path.join(d, "wayland-8.md"), "w").write(feature)
    if cargo is not None:
        open(os.path.join(root, "Cargo.toml"), "w").write(cargo)
    return root


def self_test():
    cases = []

    def case(label, must_fire, defect=_KEEP, feature=_KEEP, expect=None,
             offline=False, inj=_INJ, cargo=_KEEP):
        cases.append((label, must_fire,
                      _DEFECT if defect is _KEEP else defect,
                      _FEATURE if feature is _KEEP else feature,
                      expect, offline, inj,
                      _cargo(_FIXTURE_VERSION) if cargo is _KEEP else cargo))

    # ── the controls, both arms ─────────────────────────────────────────
    case("clean control: defect done, feature outstanding", False)
    case("control again, offline", False, offline=True)
    case("all-met tree: every criterion met, nothing outstanding", False,
         defect=_DEFECT.replace(
             "    state: blocked\n    owner: maintainer\n"
             "    handoff: \"FerroxLabs/wayland#11\"\n"
             "    note: \"needs a Slack workspace credential the core lane "
             "does not hold\"\n",
             "    state: met\n    evidence: \"file:src/t.rs\"\n"
             "    owner: maintainer\n"),
         feature=None)
    case("a met core criterion on a defect stays silent", False,
         defect=_DEFECT.replace('    state: met\n    evidence: '
                                '"test:src/t.rs::the_boundary_is_probed"',
                                '    state: met\n    evidence: "file:src/t.rs"'))

    # ── the release-blocking set ────────────────────────────────────────
    case("a not-met criterion owned by core on a DEFECT", True,
         defect=_DEFECT.replace(
             '    state: met\n    evidence: '
             '"test:src/t.rs::the_boundary_is_probed"\n',
             "    state: not-met\n"),
         expect="OUTSTANDING   c1")
    case("a not-met criterion owned by core on a FEATURE (out of scope)",
         False,
         feature=_FEATURE.replace(
             '  - id: c1\n',
             '  - id: c0\n    text: "a second unbuilt half"\n'
             '    state: not-met\n    owner: core\n  - id: c1\n'),
         expect="OK: every `kind: defect` entry")
    case("a superseded core criterion is not this gate's business", False,
         defect=_DEFECT.replace(
             '    state: met\n    evidence: '
             '"test:src/t.rs::the_boundary_is_probed"\n',
             '    state: superseded\n    note: "carried by #11"\n'))

    # ── decomposition ───────────────────────────────────────────────────
    case("a blocked non-core criterion with a handoff", False)
    case("a blocked non-core criterion with NO handoff", True,
         defect=_DEFECT.replace(
             '    handoff: "FerroxLabs/wayland#11"\n', ""),
         expect="UNDECOMPOSED  c2")
    case("a not-met criterion owned by desktop WITH a handoff", False,
         defect=_DEFECT.replace(
             "    state: blocked\n    owner: maintainer",
             "    state: not-met\n    owner: desktop"))
    case("a not-met criterion owned by desktop with NO handoff", True,
         defect=_DEFECT.replace(
             "    state: blocked\n    owner: maintainer",
             "    state: not-met\n    owner: desktop").replace(
             '    handoff: "FerroxLabs/wayland#11"\n', ""),
         expect="UNDECOMPOSED  c2")
    case("a handoff that is a bare issue number", True,
         defect=_DEFECT.replace('"FerroxLabs/wayland#11"', '"#11"'),
         expect="is not `<owner>/<repo>#<number>`")
    case("a handoff naming an issue that is CLOSED", True,
         inj={"labels": _INJ["labels"],
              "issues": {"FerroxLabs/wayland#11": "closed"}},
         expect="is CLOSED")
    case("a handoff naming an issue that does not exist", True,
         inj={"labels": _INJ["labels"], "issues": {}},
         expect="does not exist")
    case("a bad handoff is invisible offline, and the run SAYS so", False,
         offline=True,
         inj={"labels": _INJ["labels"], "issues": {}},
         expect="THIS IS NOT A PASS")

    # ── classification ──────────────────────────────────────────────────
    # ── release scope ───────────────────────────────────
    # A filter that REMOVES issues from the blocking set is the most
    # dangerous change this file can carry: every mistake it makes is
    # silent and points the same way, toward green. Exercised in both
    # directions, on the SAME mutated defect the blocking arm above uses.
    # The default `_DEFECT` is the clean control and goes green for reasons
    # unrelated to milestones -- these arms were written against it once
    # and all three failed, the control included.
    # ── a CLOSED handoff carrier: finished vs abandoned vs unledgered ───
    # The rule used to fail on ANY closed carrier, which reds a correctly
    # completed decomposition forever. These three fix the polarity and pin it.
    case("a CLOSED carrier that still owes work on its own ledger", True,
         defect=_DEFECT.replace('    handoff: "FerroxLabs/wayland#11"',
                               '    handoff: "FerroxLabs/wayland#8"'),
         inj={"labels": _INJ["labels"],
              "issues": {"FerroxLabs/wayland#8": "closed"}},
         expect="still owes work")
    case("a CLOSED carrier that owes NOTHING is a finished residual", False,
         defect=_DEFECT.replace('    handoff: "FerroxLabs/wayland#11"',
                               '    handoff: "FerroxLabs/wayland#8"'),
         feature=_FEATURE.replace("    state: not-met\n",
                                  '    state: met\n    evidence: "file:src/t.rs"\n'),
         inj={"labels": _INJ["labels"],
              "issues": {"FerroxLabs/wayland#8": "closed"}})
    case("a CLOSED carrier with NO ledger is still a hole", True,
         inj={"labels": _INJ["labels"],
              "issues": {"FerroxLabs/wayland#11": "closed"}},
         expect="has no ledger")

    case("an OPEN defect owing work with NO milestone", True,
         defect=_DEFECT.replace(
             '    state: met\n    evidence: '
             '"test:src/t.rs::the_boundary_is_probed"\n',
             "    state: not-met\n"),
         inj={"labels": _INJ["labels"], "issues": _INJ["issues"],
              "milestones": {"FerroxLabs/wayland#7": None}},
         expect="carries NO milestone")
    _OWES = _DEFECT.replace(
        '    state: met\n    evidence: '
        '"test:src/t.rs::the_boundary_is_probed"\n',
        "    state: not-met\n")
    _MS_NEXT = {"labels": _INJ["labels"], "issues": _INJ["issues"],
                "milestones": {"FerroxLabs/wayland#7": _FIXTURE_NEXT}}
    _MS_THIS = {"labels": _INJ["labels"], "issues": _INJ["issues"],
                "milestones": {"FerroxLabs/wayland#7": _FIXTURE_VERSION}}
    case("an OPEN defect milestoned to a LATER release does not block",
         False, defect=_OWES, inj=_MS_NEXT,
         expect="not `%s`" % _FIXTURE_VERSION)
    case("control: that same defect in THIS release still blocks", True,
         defect=_OWES, inj=_MS_THIS, expect="OUTSTANDING   c1")

    # ── the milestone is DERIVED, and it TRACKS the version ─────────────
    # An A/B on exactly one variable. Both arms carry the identical ledger
    # (#7 owes a core criterion) and the identical tracker state (#7 is
    # milestoned 9.9.10). The ONLY difference is the version in the
    # fixture's Cargo.toml. Green then red is the whole claim: the scope
    # followed the bump. If it did not, both arms land the same way and
    # neither of them means anything -- which is precisely what the
    # hardcoded constant did to this gate for a whole release.
    case("tree at 9.9.9: a 9.9.10 defect is the NEXT release's problem",
         False, defect=_OWES, inj=_MS_NEXT, cargo=_cargo(_FIXTURE_VERSION),
         expect="not `%s`" % _FIXTURE_VERSION)
    case("bump the tree to 9.9.10: the SAME defect now blocks",
         True, defect=_OWES, inj=_MS_NEXT, cargo=_cargo(_FIXTURE_NEXT),
         expect="RELEASE BLOCKED (%s)" % _FIXTURE_NEXT)
    # And the release candidate grades the release it is a candidate FOR.
    # There is no `9.9.10-rc.2` milestone on any tracker; a gate that looked
    # for one would find nothing in scope and pass an rc on emptiness.
    case("a 9.9.10-rc.2 tree grades the 9.9.10 milestone", True,
         defect=_OWES, inj=_MS_NEXT,
         cargo=_cargo(_FIXTURE_NEXT + "-rc.2"),
         expect="RELEASE BLOCKED (%s)" % _FIXTURE_NEXT)

    # ── derivation failures are LOUD ────────────────────────────────────
    # Each of these used to be impossible because the answer was typed in.
    # Now the answer is read, so every way of failing to read it has to end
    # the run. A fallback here would re-create the original defect exactly:
    # a gate quietly grading a release nobody is cutting.
    case("no Cargo.toml at all", True, cargo=None,
         expect="has no default to fall back on")
    case("a Cargo.toml with no [workspace.package] table", True,
         cargo='[workspace]\nresolver = "2"\nversion = "0.0.0-decoy"\n',
         expect="under `[workspace.package]`")
    case("a workspace version that is not a version", True,
         cargo=_cargo("not-a-version"),
         expect="does not reduce to an X.Y.Z milestone")
    case("a two-component workspace version", True, cargo=_cargo("0.13"),
         expect="does not reduce to an X.Y.Z milestone")
    case("an empty workspace version string", True, cargo=_cargo(""),
         expect="does not reduce to an X.Y.Z milestone")

    case("a ledger entry with no `kind:` field", True,
         defect=_DEFECT.replace("kind: defect\n", ""),
         expect="no `kind:` field")
    case("a `kind:` that is neither defect nor feature", True,
         defect=_DEFECT.replace("kind: defect", "kind: chore"),
         expect="must be one of defect/feature/task")
    case("`kind: feature` on an issue GitHub labels `bug`", True,
         feature=_FEATURE.replace("kind: feature", "kind: feature"),
         inj={"labels": {"FerroxLabs/wayland#7": ["bug"],
                         "FerroxLabs/wayland#8": ["bug", "area:core"]},
              "issues": {"FerroxLabs/wayland#11": "open"}},
         expect="MISCLASSIFIED")
    # ── `kind: task`: excluded from blocking, and NOT an escape hatch ───
    # A task is a ticket whose every remaining criterion is a credential or an
    # account a human must obtain. It must be excluded (or the gate can never
    # go green), and it must be impossible to abuse (or every awkward defect
    # becomes a task). Both directions are proven here.
    case("`kind: task` with a not-met CORE criterion does not block", False,
         feature=_FEATURE.replace("kind: feature", "kind: task").replace(
             '  - id: c1\n',
             '  - id: c0\n    text: "a credential nobody can issue"\n'
             '    state: not-met\n    owner: core\n  - id: c1\n'),
         inj={"labels": {"FerroxLabs/wayland#7": ["bug"],
                         "FerroxLabs/wayland#8": []},
              "issues": {"FerroxLabs/wayland#11": "open"}},
         expect="OK: every `kind: defect` entry")
    case("`kind: task` on an issue GitHub labels `bug` is MISCLASSIFIED", True,
         feature=_FEATURE.replace("kind: feature", "kind: task"),
         inj={"labels": {"FerroxLabs/wayland#7": ["bug"],
                         "FerroxLabs/wayland#8": ["bug", "area:core"]},
              "issues": {"FerroxLabs/wayland#11": "open"}},
         expect="MISCLASSIFIED")
    case("every `kind: task` is NAMED, because none can be corroborated",
         False,
         feature=_FEATURE.replace("kind: feature", "kind: task"),
         inj={"labels": {"FerroxLabs/wayland#7": ["bug"],
                         "FerroxLabs/wayland#8": ["area:core"]},
              "issues": {"FerroxLabs/wayland#11": "open"}},
         expect="[kind: task]")

    case("`kind: feature` that no label corroborates is NAMED, not failed",
         False,
         inj={"labels": {"FerroxLabs/wayland#7": ["bug"],
                         "FerroxLabs/wayland#8": []},
              "issues": {"FerroxLabs/wayland#11": "open"}},
         expect="no tracker label corroborates")
    case("`kind: defect` on an issue GitHub labels `enhancement` still blocks",
         False,
         inj={"labels": {"FerroxLabs/wayland#7": ["enhancement"],
                         "FerroxLabs/wayland#8": ["enhancement"]},
              "issues": {"FerroxLabs/wayland#11": "open"}},
         expect="worth a second look")

    # ── vacuity ─────────────────────────────────────────────────────────
    case("a tree in which nothing at all is a defect", True,
         defect=None, expect="not one ledger entry is `kind: defect`")
    case("a ledger file this gate cannot parse", True,
         defect=_DEFECT.replace("---\n\nProse", "\nProse"),
         expect="does not parse")

    ok = True
    results = []
    for label, must, d, f, expect, offline, inj, cg in cases:
        # An arm whose fixture is byte-identical to the control, and whose
        # tracker state is the control's too, is testing the control. One arm
        # in this file did exactly that during development and read as a pass.
        # The manifest is in the comparison because several arms below mutate
        # nothing else, and without it they would read as untested control.
        if (must and d == _DEFECT and f == _FEATURE and inj is _INJ
                and cg == _cargo(_FIXTURE_VERSION)):
            print("  %-58s MUTATION DID NOT APPLY -- the arm tests nothing"
                  % label[:58])
            ok = False
            continue
        with tempfile.TemporaryDirectory() as td:
            _fixture(td, defect=d, feature=f, cargo=cg)
            code, out = run(td, offline=offline, injected=inj)
        fired = code != 0
        good = fired == must
        if good and expect and expect not in "\n".join(out):
            good = False
            print("  %-58s did not say what it was written to say (%r absent)"
                  % (label[:58], expect))
        ok &= good
        results.append((label, must, fired, good))

    # Scanning nothing must never pass, twice over.
    for label, setup in (
        ("no ledger directory at all", lambda td: None),
        ("a ledger directory holding no files",
         lambda td: os.makedirs(os.path.join(td, ".planning", "ledger"))),
    ):
        with tempfile.TemporaryDirectory() as td:
            # A readable manifest, so these arms fail on the ledger and not on
            # the derivation. A red for somebody else's reason proves nothing
            # about the check it was written for.
            open(os.path.join(td, "Cargo.toml"), "w").write(
                _cargo(_FIXTURE_VERSION))
            setup(td)
            code, _ = run(td, offline=True)
        results.append((label, True, code != 0, code != 0))
        ok &= code != 0

    # And the control once more, after every mutation arm, so a fixture that
    # leaked state between arms shows up as a red control rather than as a
    # silently weaker gate.
    with tempfile.TemporaryDirectory() as td:
        _fixture(td, _DEFECT, _FEATURE, _cargo(_FIXTURE_VERSION))
        code, _ = run(td, injected=_INJ)
    results.append(("control after the vacuity arms (still green)",
                    False, code != 0, code == 0))
    ok &= code == 0

    for label, must, got, good in results:
        print("  %-58s expected %-5s got %-5s  %s"
              % (label[:58], "RED" if must else "green",
                 "RED" if got else "green", "ok" if good else "SELF-TEST FAILED"))
    print("self-test: %s"
          % ("both directions proven" if ok
             else "BROKEN -- the gate cannot be trusted"))
    return 0 if ok else 1


def main(argv):
    root = os.path.dirname(HERE)
    code, out = run(root, offline="--offline" in argv)
    print("\n".join(out))
    return code


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(main(sys.argv))
