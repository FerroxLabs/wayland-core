#!/usr/bin/env python3
"""One red must measure one thing (FerroxLabs/wayland-core#412 c3).

`ci-linux` runs a battery of INDEPENDENT gate checks. GitHub Actions skips every
later step when one fails, so on 2026-08-31 a redaction violation in a single
ledger file took out twenty-four of them -- the nextest run, fmt, clippy, both
release gates, contract-corpus drift and the security audit -- on the branch that
was about to ship. The job did go red, so nothing was silently green; what was
lost was the MEASUREMENT, and the one red everyone read blamed a ledger file.

A one-time edit does not hold: the NEXT step somebody appends without a condition
re-creates the suppression for everything after it, and no gate would notice. So
this check FAILS CLOSED -- a step with no condition is a defect unless it is named
here with the reason its failure genuinely invalidates what follows.
"""
import io, re, sys

WORKFLOW = ".github/workflows/ci.yml"
JOB = "  ci-linux:"

# Steps whose failure means the steps after them CANNOT produce a meaningful
# verdict -- setup, disk, and the image every later step runs inside. Suppression
# is correct here; the reason is recorded so the exemption is a decision.
# `Free disk space` and `Build CI image` were exempt here for exactly one run, and
# it was WRONG -- measured on the core#412 proof run. Neither depends on the checks
# above it, so exempting them let one redaction violation skip the image and take
# `Run tests`, `Clippy`, `Check formatting` and `Security audit` down with it: four
# of the twenty-four steps this issue is about. Exemption is for a step whose
# failure makes the later ones UNMEASURABLE, never for one that merely comes first.
# The three pre-build steps were exempt as "build prerequisites" and that was
# WRONG for the same reason the image was: a prerequisite that does not run
# produces no artifact, and the step that needs it then RUNS and fails for want
# of it. MEASURED on core#412 run 33417001379 -- `Release binary smoke` failed
# in CI while passing 27/27 locally, because `Pre-build wcore-cli release binary`
# had been skipped. A prerequisite must run, not be excused.
SUPPRESSIBLE = {
    "Report macOS runner budget": "reads inputs; nothing downstream can be graded if it cannot",
    # NOT because its failure makes later steps unmeasurable -- it does not -- but
    # because `wcore-protocol::contract_gate_topology::the_corpus_drift_step_is_a_gate_
    # and_carries_nothing_that_could_silence_it` forbids ANY `if:` on it, including
    # `!cancelled()`. That invariant is older and explicit: ask #2 narrowed the corpus
    # gate to one standalone step, and a condition on it removes that coverage QUIETLY.
    # Two invariants about the same step, and they genuinely conflict; the specific and
    # pre-existing one wins over this general rule. Recorded rather than resolved, so
    # whoever revisits either sees both.
    "Check Desktop protocol contract corpus drift": "its own gate-topology test forbids any if:",
}
NON_SUPPRESSING = ("!cancelled()", "always()")


def job_block(lines):
    start = next(i for i, l in enumerate(lines) if l.startswith(JOB))
    for i in range(start + 1, len(lines)):
        if re.match(r"^  [a-zA-Z0-9_-]+:\s*$", lines[i]):
            return start, i
    return start, len(lines)


def audit(text):
    lines = text.splitlines()
    start, end = job_block(lines)
    problems, checked = [], 0
    for i in range(start, end):
        m = re.match(r"^(\s+)- name: (.*)$", lines[i])
        if not m:
            continue
        name = m.group(2).strip()
        cond = ""
        for j in range(i + 1, min(i + 8, end)):
            if re.match(r"^\s+- name: ", lines[j]):
                break
            c = re.match(r"^\s+if: (.*)$", lines[j])
            if c:
                cond = c.group(1)
                break
        exempt = next((k for k in SUPPRESSIBLE if name.startswith(k)), None)
        if exempt:
            continue
        checked += 1
        if not any(tok in cond for tok in NON_SUPPRESSING):
            problems.append(
                "%s: step %r has condition %r. A failure ABOVE it in ci-linux will "
                "skip it, so its verdict is lost and the one red on the job is "
                "attributed to something else. Give it `if: ${{ !cancelled() }}` "
                "(add `&& steps.ci_image.outcome == 'success'` if it runs inside the "
                "image), or name it in SUPPRESSIBLE with the reason its failure "
                "genuinely invalidates what follows."
                % (WORKFLOW, name, cond or "<none>"))
    return problems, checked


def self_test():
    src = io.open(WORKFLOW, encoding="utf-8").read()
    ok = True

    def arm(label, mutate, must_fire, expect=None):
        nonlocal ok
        body = mutate(src)
        if mutate is not (lambda s: s) and body == src and must_fire:
            print("  %-56s MUTATION DID NOT APPLY" % label[:56]); ok = False; return
        probs, _ = audit(body)
        fired = bool(probs)
        good = fired == must_fire
        if good and expect and not any(expect in p for p in probs):
            good = False
            print("  %-56s fired, but not for its own reason" % label[:56])
        elif good:
            print("  %-56s expected %-5s got %-5s ok"
                  % (label[:56], "RED" if must_fire else "green", "RED" if fired else "green"))
        else:
            print("  %-56s expected %-5s got %-5s FAIL"
                  % (label[:56], "RED" if must_fire else "green", "RED" if fired else "green"))
            for p in probs[:2]:
                print("      %s" % p[:150])
        ok = ok and good

    arm("the tree as it stands", lambda s: s, False)
    # A green arm that passes because it checked NOTHING is the vacuity one rung down.
    _, n = audit(src)
    if n < 15:
        print("  %-56s only %d step(s) graded -- too few to mean anything"
              % ("coverage: the audit actually reaches the steps", n)); ok = False
    else:
        print("  %-56s %d steps graded  ok" % ("coverage: the audit actually reaches the steps", n))
    arm("an independent check loses its condition",
        lambda s: s.replace(
            "      - name: Criteria ledger is anchored and parseable\n"
            "        if: ${{ !cancelled() }}\n",
            "      - name: Criteria ledger is anchored and parseable\n", 1),
        True, expect="Criteria ledger is anchored and parseable")
    arm("a build-dependent check loses its guard",
        lambda s: s.replace(
            "      - name: Security audit\n"
            "        if: ${{ !cancelled() && steps.ci_image.outcome == 'success' }}\n",
            "      - name: Security audit\n", 1),
        True, expect="Security audit")
    # Anchored on the GUARDED form, which is unique to ci-linux. Anchoring on the
    # bare step name inserted into a different job, and the arm passed by testing
    # nothing -- the same vacuity this file exists to catch, one level up.
    AUDIT = ("      - name: Security audit\n"
             "        if: ${{ !cancelled() && steps.ci_image.outcome == 'success' }}\n")
    arm("a NEW step is appended with no condition",
        lambda s: s.replace(
            AUDIT, "      - name: Some later gate somebody adds\n        run: echo hi\n" + AUDIT, 1),
        True, expect="Some later gate somebody adds")
    arm("a new step that IS exempt, with its reason on file",
        lambda s: s.replace(
            AUDIT, "      - name: Report macOS runner budget (second pass)\n        run: echo hi\n" + AUDIT, 1),
        False)
    print("self-test: both directions proven" if ok else "self-test: FAILED")
    return 0 if ok else 1


def main():
    if "--self-test" in sys.argv:
        return self_test()
    probs, checked = audit(io.open(WORKFLOW, encoding="utf-8").read())
    print("ci-linux: %d step(s) graded for suppression" % checked)
    if probs:
        print("\nFAIL: %d step(s) can be silently skipped by a failure above them:" % len(probs))
        for p in probs:
            print("  " + p)
        return 1
    print("OK: no step in ci-linux reports on behalf of steps that never ran.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
