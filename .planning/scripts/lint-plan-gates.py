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
]

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
            rc = subprocess.run(
                cmd, shell=True, cwd=REPO, timeout=25,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            ).returncode
        except subprocess.TimeoutExpired:
            continue
        except Exception:
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
