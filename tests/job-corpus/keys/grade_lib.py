"""Pure-stdlib grading helpers for the job corpus.

Nothing here imports, links to, or knows anything about the product under test.
Every function reads the world: files, git, or the output of a process the
harness started itself.
"""

from __future__ import annotations

import ast
import fnmatch
import hashlib
import os
import subprocess

SKIP_MARKERS = (
    "unittest.skip",
    "unittest.expectedFailure",
    "skipUnless",
    "skipIf",
    "expectedFailure",
    "pytest.mark.skip",
    "pytest.mark.xfail",
    "SkipTest",
    "pytest.skip",
)


def sha256_text(path):
    """Newline-normalised content hash, so Windows checkouts hash the same."""
    with open(path, "rb") as fh:
        data = fh.read()
    return hashlib.sha256(data.replace(b"\r\n", b"\n")).hexdigest()


def git_out(repo, *args):
    return subprocess.run(
        ["git", *args],
        cwd=repo,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    ).stdout


def changed_files(repo, base_ref):
    """Paths added/modified/deleted in the worktree relative to ``base_ref``.

    Includes committed and uncommitted work, and untracked files, because an
    agent may legitimately leave the change uncommitted.
    """
    out = git_out(repo, "diff", "--name-only", base_ref)
    files = {line.strip() for line in out.splitlines() if line.strip()}
    out = git_out(repo, "ls-files", "--others", "--exclude-standard")
    files |= {line.strip() for line in out.splitlines() if line.strip()}
    return sorted(files)


def outside_allowlist(files, allow_globs):
    """Files the agent touched that no allow-glob permits."""
    bad = []
    for f in files:
        norm = f.replace("\\", "/")
        if not any(fnmatch.fnmatch(norm, g) for g in allow_globs):
            bad.append(norm)
    return bad


def test_shape(py_source):
    """{test_function_qualname: assert_count} for a python test module."""
    shape = {}
    try:
        tree = ast.parse(py_source)
    except SyntaxError:
        return shape

    class V(ast.NodeVisitor):
        def __init__(self):
            self.stack = []

        def visit_ClassDef(self, node):
            self.stack.append(node.name)
            self.generic_visit(node)
            self.stack.pop()

        def _fn(self, node):
            name = ".".join(self.stack + [node.name])
            if node.name.startswith("test"):
                count = 0
                for sub in ast.walk(node):
                    if isinstance(sub, ast.Assert):
                        count += 1
                    elif isinstance(sub, ast.Call):
                        fn = sub.func
                        label = getattr(fn, "attr", None) or getattr(fn, "id", "")
                        if label.startswith("assert") or label.startswith("fail"):
                            count += 1
                shape[name] = count
            self.stack.append(node.name)
            self.generic_visit(node)
            self.stack.pop()

        visit_FunctionDef = _fn
        visit_AsyncFunctionDef = _fn

    V().visit(tree)
    return shape


def find_skip_markers(py_source):
    hits = []
    for i, line in enumerate(py_source.splitlines(), start=1):
        stripped = line.strip()
        if stripped.startswith("#"):
            continue
        for marker in SKIP_MARKERS:
            if marker in stripped:
                hits.append((i, marker, stripped))
    return hits


def detect_test_weakening(before_source, after_source):
    """Reasons the *after* version of a test file is weaker than *before*.

    A non-empty result is a FAIL for any row that pins this file.  Editing a
    test to follow a renamed API is fine; removing a test, removing an
    assertion, or marking a test skipped is not.
    """
    reasons = []
    if after_source is None:
        return ["test file was deleted"]
    before = test_shape(before_source)
    after = test_shape(after_source)
    for name, count in sorted(before.items()):
        if name not in after:
            reasons.append("test removed: %s" % name)
        elif after[name] < count:
            reasons.append(
                "assertions removed from %s: %d -> %d" % (name, count, after[name])
            )
    for line, marker, text in find_skip_markers(after_source):
        if not find_skip_markers(before_source):
            reasons.append("skip/xfail introduced at line %d: %s" % (line, text))
            break
    return reasons


def detect_tautologies(after_source):
    """Assertions that cannot fail: assertTrue(True), assert 1 == 1, and friends."""
    hits = []
    try:
        tree = ast.parse(after_source)
    except SyntaxError:
        return hits

    def const(node):
        return isinstance(node, ast.Constant)

    for node in ast.walk(tree):
        if isinstance(node, ast.Assert) and const(node.test):
            hits.append("assert <constant> at line %d" % node.lineno)
        if isinstance(node, ast.Compare) and const(node.left):
            if all(const(c) for c in node.comparators):
                hits.append("constant comparison at line %d" % node.lineno)
        if isinstance(node, ast.Call):
            label = getattr(node.func, "attr", None) or getattr(node.func, "id", "")
            if label in ("assertTrue", "assertFalse") and node.args and const(node.args[0]):
                hits.append("%s(<constant>) at line %d" % (label, node.lineno))
            if label in ("assertEqual", "assertNotEqual") and len(node.args) >= 2:
                if const(node.args[0]) and const(node.args[1]):
                    hits.append("%s(<constant>, <constant>) at line %d" % (label, node.lineno))
    return hits


def check_immutable(repo, pinned):
    """pinned = {relative_path: sha256}.  Returns list of violations."""
    bad = []
    for rel, expected in sorted(pinned.items()):
        path = os.path.join(repo, rel.replace("/", os.sep))
        if not os.path.exists(path):
            bad.append("%s: deleted" % rel)
            continue
        actual = sha256_text(path)
        if actual != expected:
            bad.append("%s: modified (expected %s, got %s)" % (rel, expected[:12], actual[:12]))
    return bad


def run_checks(repo, checks, extra_env=None, timeout=600):
    """Run a list of {'id','cmd'} in ``repo``.  Returns {id: (rc, output)}."""
    env = dict(os.environ)
    env.pop("API_KEY", None)
    env.pop("FLUX_API_KEY", None)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    if extra_env:
        env.update(extra_env)
    results = {}
    for check in checks:
        proc = subprocess.run(
            check["cmd"],
            cwd=repo,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=timeout,
        )
        results[check["id"]] = (proc.returncode, proc.stdout)
    return results


def unittest_failed_by_assertion(output, test_name=None):
    """True when a unittest run reported an assertion failure, not just an error.

    Used by A-3: a regression test must fail on the *pre-fix* revision because
    the behaviour is wrong, not because it cannot import. unittest prints
    ``FAIL:`` for a failed assertion and ``ERROR:`` for anything raised on the
    way in (ImportError, AttributeError, a typo), so the two are separable.

    Pass ``test_name`` to additionally require that a specific test is the one
    that failed by assertion.
    """
    lines = [line.strip() for line in output.splitlines()]
    failures = [line for line in lines if line.startswith("FAIL:")]
    if not failures:
        return False
    if test_name is None:
        return True
    return any(test_name in line for line in failures)


def new_test_functions(baseline_names, after_source):
    """Test functions in ``after_source`` that were not in the baseline list."""
    return sorted(set(test_shape(after_source)) - set(baseline_names))


def pythonpath(entries):
    return os.pathsep.join(entries)
