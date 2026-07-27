#!/usr/bin/env python3
"""Lint PLAN.md verification gates for shapes that cannot go red.

Three adversarial review rounds on phases 23A/24/26/27 each found the same
defect wearing a new costume: a gate that reports success no matter what the
executor does. Prose instructions did not stop it, because "can this go red?"
is a judgment made dozens of times per plan. This makes it mechanical.

Two classes of check:

  STATIC   pattern-match gate text for shapes that are unconditionally green
           (a pipe that swallows the exit status, `git status --porcelain`
           with no `test -z`, grepping a file the plan itself writes, ...).

  BASELINE run each safely-runnable gate against the UNTOUCHED tree. A gate
           that is already green before any work happens cannot signal that
           the work was done. This is the class the round-3 checker had to
           find by hand, and it is the most expensive one: it looks like
           coverage and is worth nothing.

Only read-only gates are executed. Anything touching ssh/cargo/nextest/rm/
git-write is reported as NOT-RUN rather than guessed at, because a wrong
verdict here is worse than an absent one.

Usage:
    lint-plan-gates.py <plan-file-or-directory>...
    lint-plan-gates.py --static-only <paths>     # skip baseline execution

Exit status: 1 if any HIGH finding, else 0.
"""

import html
import os
import re
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# Gate text lives in <automated>...</automated> blocks inside <verify> sections.
AUTOMATED_RE = re.compile(r"<automated>(.*?)</automated>", re.DOTALL)

# Commands we refuse to execute: side effects, remote hosts, or minutes-long builds.
UNSAFE_RE = re.compile(
    r"\b(ssh|scp|cargo|nextest|vx|rm|mv|cp|npm|node|pwsh|powershell|schtasks|"
    r"git\s+(add|commit|push|checkout|reset|stash|rebase|worktree|fetch|clone))\b"
)

STATIC_RULES = [
    (
        "HIGH",
        "pipe-steals-exit-status",
        re.compile(r"\b(ssh|curl|wget)\b[^|]*\|(?!\|)"),
        "The pipeline's exit status is the LAST command's, not the one you care about. "
        "A failed remote command still exits 0. Use `cmd > out; rc=$?; test $rc -eq 0` "
        "or append `; exit $LASTEXITCODE` inside the remote shell.",
    ),
    (
        "HIGH",
        "git-status-always-green",
        re.compile(r"git\s+(status\s+--porcelain|diff\s+--stat)(?!.*\|)"),
        "`git status --porcelain` and `git diff --stat` exit 0 unconditionally, dirty or "
        "clean. Use `test -z \"$(git status --porcelain -- <paths>)\"`.",
    ),
    (
        "HIGH",
        "test-s-only",
        re.compile(r"test\s+-s\s+\S+(\s*&&\s*test\s+-s\s+\S+)*\s*&&\s*diff\b"),
        "`test -s` before `diff` reddens only on an empty file. Content, coverage and "
        "freshness are all unconstrained. Assert a line-count floor and a run marker "
        "tied to the pinned SHA.",
    ),
    (
        "MEDIUM",
        "name-only-blind-to-untracked",
        re.compile(r"git\s+diff\s+--name-only"),
        "`git diff --name-only` cannot see UNTRACKED files, so a stray new file never "
        "trips it, and it goes permanently green once committed. Pair it with a "
        "working-tree leg.",
    ),
    (
        "HIGH",
        "unquoted-pathspec-shell-dependent",
        re.compile(r"\bgit\s+(?:diff|ls-files)\b[^\n|;]*?--\s+\$[A-Za-z_][A-Za-z0-9_]*"),
        "An unquoted `$VAR` holding SPACE-SEPARATED paths only splits into multiple pathspecs "
        "under a shell that word-splits. **zsh does not**, so the whole string becomes ONE "
        "pathspec, it matches nothing, and `--quiet` exits 0 no matter what changed. Measured "
        "live: a seam gate printed SEAM CLEAN while Cargo.lock carried 3 added lines. Quoting "
        "it does NOT fix this -- that makes it one pathspec everywhere. Inline the paths "
        "literally instead. This is the only gate defect here whose correctness depends on "
        "which shell the operator happens to use.",
    ),
    # ---- The mirror class: gates that cannot go GREEN. -------------------
    # Everything above catches a gate that always passes. These catch a gate
    # that always fails, which is just as useless and much more confusing --
    # it burns an executor's time proving a defect that is in the gate.
    # All three were found BY HAND during Phase 29 planning, after this
    # linter reported that plan set clean.
    (
        "HIGH",
        "grep-rc-prefixes-the-count",
        # Must be a real option CLUSTER: whitespace, a dash, then letters only,
        # containing both r and c. The naive `-\w*r\w*c` also matched hyphenated
        # words inside PATHS -- `.../28-native-cross-platform-certification/`
        # contains `-certification`, which is `-` + `ce` + `r` + `tifi` + `c`.
        # That fired on six clean gates across phases 28 and 29 on first run.
        re.compile(r"\bgrep\b[^\n|;]*?\s-(?=[A-Za-z]*[rR])(?=[A-Za-z]*c)[A-Za-z]+(?=\s|$)"),
        "Recursive `grep -c` prints `path:count` PER FILE, not a bare number. So "
        "`test \"$(grep -rc ...)\" -eq 0` compares the string `path:0` against 0 and dies "
        "with 'integer expression expected' every single time, pass or fail. Use "
        "`grep -rho ... | wc -l`, or `grep -rc ... | cut -d: -f2`.",
    ),
    (
        "HIGH",
        "grep-c-exit-1-breaks-chain",
        re.compile(r"\w+=\"?\$\(\s*grep\b[^)]*-c\b[^)]*\)\"?\s*&&"),
        "`grep -c` exits 1 when the count is ZERO, and a command substitution assignment "
        "takes that exit status. So this `&&` chain breaks precisely when the count is 0 -- "
        "which is usually the PASSING condition. Split the assignment off the chain, or "
        "append `|| true` to the substitution.",
    ),
    (
        "MEDIUM",
        "backslash-s-not-portable",
        re.compile(r"grep\b(?![^\n|;]*-P\b)[^\n|;]*\\s"),
        "`\\s` is a GNU extension. BSD/macOS grep matches a literal 's' instead, so this "
        "gate means different things on the two hosts this program builds on. Use a POSIX "
        "class -- `[[:space:]]` -- or pass -P where PCRE is guaranteed.",
    ),
]

# stderr signatures that mean the gate is BROKEN rather than legitimately red.
# A gate SHOULD fail against the untouched tree -- that is the whole point. But
# it should fail by asserting something absent, not by being malformed. Anything
# here indicates the shell could not even run the check.
#
BROKEN_GATE_STDERR = re.compile(
    r"integer expression expected|unary operator expected|syntax error|"
    r"command not found|invalid option|unknown option|unrecognized option|"
    r"Try '.*--help'|conditional binary operator expected",
    re.IGNORECASE,
)

# ...but ONLY when the error is not downstream of a missing artifact. The
# canonical correct gate here is `test "$(grep -c X evidence/foo.log)" -eq 3`,
# which at base emits BOTH "No such file or directory" AND, because the
# substitution is then empty, "integer expression expected". That gate is
# perfectly good -- it goes green the moment the executor produces the file.
# Without this suppression the check fired on six such gates across phases 28
# and 29, i.e. it flagged the very shape the linter wants people to write.
MISSING_ARTIFACT_STDERR = re.compile(
    r"No such file or directory|Is a directory|cannot open|does not exist",
    re.IGNORECASE,
)

# A gate that greps a document the plan itself produces is a tautology.
SELF_WRITTEN_RE = re.compile(r"grep[^\n]*?([A-Za-z0-9_.\-/]*\d{2}[A-Z]?-\d{2}-[A-Z-]+\.md)")


# An <automated> block is a short list of shell commands. When one runs to dozens
# of lines it means the opening tag was never closed and the non-greedy match ran
# on to the NEXT block's closer, swallowing frontmatter and prose. Linting that as
# gate text produces a flood of nonsense findings, and it inflated an early
# measurement of this very repo by roughly 3x. Treat it as the plan defect it is.
MALFORMED_BLOCK_LINES = 12


def extract_gates(path):
    with open(path, encoding="utf-8") as fh:
        text = fh.read()

    opens = len(re.findall(r"<automated>", text))
    closes = len(re.findall(r"</automated>", text))
    malformed = []
    if opens != closes:
        malformed.append(
            f"{opens} <automated> opening tag(s) but {closes} closing tag(s) -- gate "
            "text is ambiguous and anything parsed from this file is unreliable"
        )

    offsets = []
    for m in AUTOMATED_RE.finditer(text):
        line_no = text.count("\n", 0, m.start()) + 1
        body_lines = [l for l in m.group(1).strip().splitlines() if l.strip()]
        if len(body_lines) > MALFORMED_BLOCK_LINES:
            malformed.append(
                f"line {line_no}: <automated> block spans {len(body_lines)} lines, "
                "which almost always means an unclosed tag above it swallowed prose"
            )
            continue
        for raw in m.group(1).strip().splitlines():
            # Gate text is XML-escaped inside the PLAN's <automated> element, so
            # `&&` arrives as `&amp;&amp;`. Running it un-unescaped makes every
            # baseline execution die on a literal `&amp;` and return non-zero --
            # which silently reports "no already-green gates" and looks clean.
            # That is the exact defect class this linter exists to catch.
            cmd = html.unescape(raw.strip())
            if cmd and not cmd.startswith("#"):
                offsets.append((line_no, cmd))
            line_no += 1
    return offsets, malformed


def static_findings(path, gates):
    out = []
    produced = set()
    with open(path, encoding="utf-8") as fh:
        body = fh.read()
    # Documents this plan declares it will write are fair game for the tautology check.
    for m in re.finditer(r"path:\s*(\S+\.md)", body):
        produced.add(os.path.basename(m.group(1)))

    for line_no, cmd in gates:
        for sev, rule, pattern, advice in STATIC_RULES:
            if pattern.search(cmd):
                out.append((sev, rule, path, line_no, cmd, advice))
        m = SELF_WRITTEN_RE.search(cmd)
        if m and os.path.basename(m.group(1)) in produced:
            out.append((
                "HIGH", "greps-own-evidence-file", path, line_no, cmd,
                f"Greps {os.path.basename(m.group(1))}, which this plan writes. Satisfied by "
                "the executor typing the token. Gate on the captured artifact instead.",
            ))
    return out


def baseline_findings(path, gates):
    """Run read-only gates against the untouched tree; already-green means no signal."""
    out = []
    for line_no, cmd in gates:
        if UNSAFE_RE.search(cmd):
            continue
        try:
            proc = subprocess.run(
                cmd, shell=True, cwd=REPO, timeout=25,
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
            )
            rc = proc.returncode
            err = (proc.stderr or b"").decode("utf-8", "replace")
        except subprocess.TimeoutExpired:
            continue
        except Exception:
            continue
        # A gate that cannot even be PARSED is not "red", it is broken. Catching
        # this needs stderr, which is why it is not discarded any more.
        if (rc != 0 and BROKEN_GATE_STDERR.search(err)
                and not MISSING_ARTIFACT_STDERR.search(err)):
            first = next((l for l in err.splitlines() if l.strip()), err.strip())
            out.append((
                "HIGH", "gate-is-broken-not-red", path, line_no, cmd,
                f"Fails with a shell/usage error, not an assertion: {first[:120]!r}. "
                "This gate can never go green no matter what the executor does, so it "
                "proves nothing and costs an executor a debugging cycle to discover.",
            ))
            continue
        if rc == 0:
            out.append((
                "HIGH", "already-green-at-base", path, line_no, cmd,
                "Passes against the UNTOUCHED tree, so it cannot indicate the task was "
                "done. Either it guards a pre-existing string (a regression guard, not a "
                "completion gate) or it is decorative. Assert something the task creates.",
            ))
    return out


def main(argv):
    static_only = "--static-only" in argv
    targets = [a for a in argv if not a.startswith("--")]
    if not targets:
        print(__doc__)
        return 2

    files = []
    for t in targets:
        if os.path.isdir(t):
            for root, _, names in os.walk(t):
                files.extend(os.path.join(root, n) for n in sorted(names)
                             if n.endswith("-PLAN.md"))
        elif t.endswith("-PLAN.md"):
            files.append(t)

    findings = []
    total_gates = 0
    for f in sorted(files):
        gates, malformed = extract_gates(f)
        for msg in malformed:
            findings.append(("HIGH", "malformed-automated-block", f, 0, msg,
                             "Close every <automated> tag. An unclosed one makes the gate "
                             "text ambiguous to every reader, human or tool."))
        total_gates += len(gates)
        findings += static_findings(f, gates)
        if not static_only:
            findings += baseline_findings(f, gates)

    order = {"HIGH": 0, "MEDIUM": 1, "LOW": 2}
    findings.sort(key=lambda x: (order.get(x[0], 3), x[2], x[3]))

    for sev, rule, path, line_no, cmd, advice in findings:
        rel = os.path.relpath(path, REPO)
        print(f"{sev:6} {rule:28} {rel}:{line_no}")
        print(f"       $ {cmd[:150]}")
        print(f"       -> {advice}")
        print()

    highs = sum(1 for f in findings if f[0] == "HIGH")
    print(f"{len(files)} plan(s), {total_gates} gate(s) examined: "
          f"{highs} HIGH, {len(findings) - highs} other")
    return 1 if highs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
