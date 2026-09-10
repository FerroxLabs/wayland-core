#!/usr/bin/env python3
"""Fail when TEST code reaches the operator's REAL state directory (#1298 c6).

WHY A SECOND CHECK
    `check-test-env-globals.py` catches a test that writes a process-global
    environment variable which production code in the same binary reads. This
    hazard is not that. `registry::state_dir()` falls through to
    `wcore_config::wayland_config_dir()` when no override is set, so an
    unguarded test writes signing seeds and an instance id into the operator's
    own config directory -- with no environment variable anywhere on the path.
    Measured at 6d45bb623: three `fail_closed_matrix` tests deposited
    cloud.key, container.key, local.key, ssh.key and instance-id into a
    sentinel WAYLAND_HOME while all 13 tests still reported ok. The env-var
    checker returns RC=0 identically at that commit and at the fixed one, so
    it cannot grade the criterion, and grading on it would certify against a
    check that cannot fail.

WHAT THIS CHECKS
    A test that reaches one of the ENTRY_POINTS below must hold a
    `StateDirGuard`, which redirects `state_dir()` at the thread that set it.
    The guard may be taken in the test itself, or returned by a fixture the
    test calls -- both idioms exist in this repo and both are correct.

    Reach is followed one level through helpers defined in the same file, the
    same bounded depth `check-test-env-globals.py` uses and for the same
    reason: deeper attribution starts convicting the wrong function.

WHAT IT DELIBERATELY DOES NOT DO
    It does not prove a guarded test cannot escape. `STATE_DIR_OVERRIDE` is
    thread-local, so a test that guards on its own thread and then spawns
    threads which construct backends is still reaching the real directory on
    those threads. That is a real residual and is reported as a WARNING rather
    than a failure, because the alternative -- failing every test that both
    guards and spawns -- would convict correct code.
"""

import re
import sys
from pathlib import Path

# Calls that resolve the real state directory when no override is installed.
ENTRY_POINTS = (
    "reference_backends(",
    "registry::state_dir(",
    "load_or_create_seed(",
    "load_or_create_node_seed(",
    "NodeRegistry::at_state_dir(",
)
GUARD = "StateDirGuard::set"
TEST_ATTRS = ("#[test]", "#[tokio::test]", "#[serial]", "#[serial_test::serial]")

FN_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)", re.M)


def function_blocks(text):
    """Yield (name, start_line, body) for every fn, by brace matching."""
    for match in FN_RE.finditer(text):
        name = match.group(1)
        brace = text.find("{", match.end())
        if brace < 0:
            continue
        depth, index = 0, brace
        while index < len(text):
            char = text[index]
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    break
            index += 1
        line = text.count("\n", 0, match.start()) + 1
        yield name, line, text[brace:index + 1], text[:match.start()]


def is_test(preamble):
    tail = preamble[-400:]
    return any(attr in tail for attr in TEST_ATTRS)


def scan(path):
    text = path.read_text(encoding="utf-8", errors="replace")
    if GUARD not in text and not any(entry in text for entry in ENTRY_POINTS):
        return [], []

    blocks = list(function_blocks(text))

    # Attribution follows `check-test-env-globals.py`: a helper is followed
    # only when its name is declared exactly once in the file, because a
    # colliding name convicts the wrong function. `new`, `set` and `drop`
    # collide on every line of this repository.
    declared = {}
    for name, _, _, _ in blocks:
        declared[name] = declared.get(name, 0) + 1
    unique = {name for name, count in declared.items() if count == 1}

    reaching = {name for name, _, body, _ in blocks
                if name in unique and any(entry in body for entry in ENTRY_POINTS)}
    providing = {name for name, _, body, _ in blocks if name in unique and GUARD in body}

    failures, warnings = [], []
    for name, line, body, preamble in blocks:
        if not is_test(preamble):
            continue
        direct = any(entry in body for entry in ENTRY_POINTS)
        via = {other for other in reaching
               if other != name and re.search(rf"(?<![:.\w]){re.escape(other)}\s*\(", body)}
        if not direct and not via:
            continue
        guarded = GUARD in body or any(
            re.search(rf"(?<![:.\w]){re.escape(other)}\s*\(", body) for other in providing if other != name
        )
        if not guarded:
            how = "directly" if direct else "via " + ", ".join(sorted(via))
            failures.append(
                f"{path}:{line}: test `{name}` reaches the real state directory {how} "
                f"with no {GUARD}; it will write into the operator's own config directory"
            )
        elif "thread::spawn" in body or "scope(" in body:
            warnings.append(
                f"{path}:{line}: test `{name}` guards and then spawns threads; "
                f"{GUARD} is thread-local, so spawned threads are NOT covered"
            )
    return failures, warnings


UNGUARDED_FIXTURE = """
#[test]
fn a_test_that_reaches_the_real_state_directory() {
    let _ = wcore_exec_backend::reference_backends(Default::default());
}
"""

GUARDED_FIXTURE = """
#[test]
fn a_test_that_scopes_its_state_directory() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = wcore_exec_backend::registry::StateDirGuard::set(dir.path());
    let _ = wcore_exec_backend::reference_backends(Default::default());
}
"""


def self_test():
    """Prove the gate can FAIL. A gate that cannot fail grades nothing.

    This is the same objection that keeps #1298 c6 open: the env-var checker
    answers RC=0 on both sides of the defect, so it certified nothing. Running
    this before the real scan means the RC=0 below is a result rather than an
    absence.
    """
    import tempfile

    with tempfile.TemporaryDirectory(prefix="state-dir-gate-") as tmp:
        bad = Path(tmp) / "unguarded.rs"
        good = Path(tmp) / "guarded.rs"
        bad.write_text(UNGUARDED_FIXTURE, encoding="utf-8")
        good.write_text(GUARDED_FIXTURE, encoding="utf-8")

        bad_failures, _ = scan(bad)
        good_failures, _ = scan(good)

    if len(bad_failures) != 1 or "a_test_that_reaches_the_real_state_directory" not in bad_failures[0]:
        print("SELF-TEST FAIL: the gate did not flag an unguarded reach")
        for failure in bad_failures:
            print(f"  {failure}")
        return 1
    if good_failures:
        print("SELF-TEST FAIL: the gate flagged a correctly guarded test")
        for failure in good_failures:
            print(f"  {failure}")
        return 1
    print("SELF-TEST OK: unguarded reach flagged, guarded reach accepted")
    return 0


def main():
    if "--self-test" in sys.argv:
        return self_test()
    root = Path(__file__).resolve().parent.parent
    failures, warnings, scanned = [], [], 0
    for crate in sorted((root / "crates").iterdir()):
        for base in ("tests", "src"):
            directory = crate / base
            if not directory.is_dir():
                continue
            for path in sorted(directory.rglob("*.rs")):
                scanned += 1
                found, warned = scan(path)
                failures.extend(found)
                warnings.extend(warned)

    for warning in warnings:
        print(f"WARNING {warning}")
    if failures:
        print(f"\nFAIL: {len(failures)} test(s) reach the real state directory unguarded\n")
        for failure in failures:
            print(f"  {failure}")
        print(f"\nTake `{GUARD}(tmp.path())` in the test, or call a fixture that returns one.")
        return 1
    print(f"OK: {scanned} files scanned, no unguarded state-directory reach "
          f"({len(warnings)} thread-scope warning(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
