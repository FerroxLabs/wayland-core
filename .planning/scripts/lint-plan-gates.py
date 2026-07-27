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
    (
        "HIGH",
        "powershell-missing-script-exits-zero",
        re.compile(r"(?:powershell|pwsh)\b[^\n|;]*-File\b"),
        "`powershell -File <missing.ps1>; exit $LASTEXITCODE` exits **0**. A gate whose script "
        "is absent — wrong path, not yet written, lost in a worktree switch — therefore PASSES. "
        "Measured on this repo. Every Windows gate in this program runs this way, so it is the "
        "highest-leverage version of the self-passing bug here. Assert the script exists first "
        "(`Test-Path` / `test -f`) and fail if it does not, then run it.",
    ),
    (
        "HIGH",
        "empty-equals-empty-passes",
        re.compile(r"\btest\s+\"\$\([^)]*\)\"\s*=\s*\"\$\([^)]*\)\""),
        "Comparing two command substitutions passes when BOTH produce empty output -- two "
        "empty strings are equal. So `test \"$(shasum X)\" = \"$(cat Y)\"` is unconditionally "
        "GREEN at base, when neither file exists yet. Demonstrated live at rc=0. This is the "
        "default shape for every digest and tamper-evidence check, so it is worth checking "
        "twice. Assert `test -s` on both operands FIRST, then compare.",
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
        # The path-qualified form matters: this repo's plans use `/usr/bin/grep`
        # throughout (the bare name is rtk-proxied on the Mac), and the first
        # version of this rule required a BARE `grep` after `$(`, so it missed
        # every real instance. Phase 30's hand-audit caught one this had passed.
        re.compile(r"\w+=\"?\$\(\s*(?:[\w./-]*/)?grep\b[^)]*-c\b[^)]*\)\"?\s*&&"),
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


_PS_FILE_RE = re.compile(r"(?:powershell|pwsh)\b[^\n|;]*?-File\s+([^\s;]+)")
_PS_GUARD_RE = re.compile(r"Test-Path\s+([^\s)]+)|test\s+-[fex]+\s+([^\s;]+)")


def _ps_script_guarded(cmd):
    """True when every `-File <script>` in `cmd` is guarded by a Test-Path/test -f
    on that same script EARLIER in the command.

    Matching on the basename is deliberate: the guard and the invocation routinely
    spell the path differently (`scripts\\x.ps1` vs `.\\x.ps1`), and requiring a
    byte-identical spelling would fail to recognise a correct guard -- which is the
    exact failure mode this suppression exists to prevent.
    """
    invocations = list(_PS_FILE_RE.finditer(cmd))
    if not invocations:
        return False
    for inv in invocations:
        target = os.path.basename(inv.group(1).replace("\\", "/")).strip("\"'")
        guarded = False
        for g in _PS_GUARD_RE.finditer(cmd):
            if g.start() >= inv.start():
                break  # a guard AFTER the call cannot protect it
            spelled = g.group(1) or g.group(2) or ""
            if os.path.basename(spelled.replace("\\", "/")).strip("\"'") == target:
                guarded = True
                break
        if not guarded:
            return False
    return True


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
            if not pattern.search(cmd):
                continue
            # Contextual suppression. `empty-equals-empty-passes` fires on the
            # SHAPE of the comparison, but the shape is fine once both operands
            # are known non-empty -- and asserting that is precisely the fix the
            # rule recommends. Without this, the rule flags every gate that has
            # already taken its own advice, which it did on two correct Phase 30
            # gates the first time it ran.
            if rule == "empty-equals-empty-passes" and cmd.count("test -s") >= 2:
                continue
            # Same shape of suppression, same reason. `powershell -File x.ps1` is
            # only self-passing when x.ps1 might be ABSENT; a `Test-Path` (or
            # `test -f`) guard on that same script ahead of the call is exactly
            # the fix the advice asks for, and the rule must recognise it or it
            # punishes the gate for complying. Measured: the guarded form exits
            # 94 on an absent script where the bare form exits 0.
            if rule == "powershell-missing-script-exits-zero" and _ps_script_guarded(cmd):
                continue
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

    # `already-green-at-base` asks "would this gate pass on an untouched tree?",
    # which is only a question worth asking BEFORE the plan runs. Once the plan
    # has executed, its artifacts exist, its gates legitimately pass at base, and
    # the rule fires on every one of them. Measured 2026-07-28: phases 28 and 29
    # went from 0 HIGH to 28 and 13 the moment their first plans merged, while
    # unexecuted phase 30 stayed at 0. Without this note the next reader sees
    # "28 HIGH" on a phase that is fine and starts chasing it.
    executed = [f for f in files
                if os.path.exists(f.replace("-PLAN.md", "-SUMMARY.md"))]
    agb = sum(1 for f in findings if f[1] == "already-green-at-base")
    if executed and agb:
        print()
        print(f"NOTE: {len(executed)} of {len(files)} plan(s) have a SUMMARY, i.e. have already "
              f"executed, and {agb} finding(s) are `already-green-at-base`.")
        print("      That rule is a PRE-EXECUTION check. On a plan that has run, its artifacts")
        print("      exist and its gates pass at base by construction -- these are expected, not")
        print("      defects. Re-read them only if you are re-planning. Every OTHER rule stays")
        print("      meaningful after execution.")
    return 1 if highs else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
