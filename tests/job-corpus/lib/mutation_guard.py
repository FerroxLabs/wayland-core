"""Prove a source mutation landed on executable code before trusting a result.

Background, and why this file exists at all
-------------------------------------------
A previous mutation harness in this project declared two perfectly good tests
"vacuous". The mutation had been applied by a single-line textual search, and
the line it matched was a **doc comment that quoted the call** rather than the
call itself. The code never changed, so of course no test noticed, and the
harness blamed the tests.

A mutation that does not change the program cannot grade anything. So every
mutation applied through this module must clear two *independent* checks, and
both must pass before any survived/killed verdict is believed:

1. **Positional**: at least one line the edit touched is a line the *compiler*
   emitted instructions for. The compiler is the authority, not a text search.
   Documentation is then subtracted from that set, which matters for the module
   docstring: it really does compile to a store into `__doc__`, so bytecode
   alone would wrongly call it code.

2. **Semantic**: the module's compiled *code fingerprint* changes. The
   fingerprint is built from the bytecode, names, varnames and constants of
   the module code object and every nested code object, and it deliberately
   excludes filenames and line-number tables -- otherwise inserting a blank
   line or a comment would shift `co_firstlineno` and masquerade as a real
   change.

Check 1 alone would accept a docstring rewrite (the parser does attribute a
line to it, and `co_consts` really does change). Check 2 alone would accept a
docstring rewrite for the same reason. Check 1 alone would reject nothing for
a comment; check 2 catches it. Together they close both holes: a docstring
edit fails check 1, a comment edit fails check 2.

`VacuousMutation` is raised, never returned as a verdict. A mutation that
cannot be proven to have landed is a **harness error**, not a test failure --
reporting it as "the tests missed it" is the bug this module exists to
prevent.
"""

import ast
import dis

__all__ = [
    "VacuousMutation",
    "executable_lines",
    "documentation_lines",
    "code_fingerprint",
    "apply_mutation",
]


class VacuousMutation(Exception):
    """The mutation did not demonstrably change executable behaviour.

    This is a defect in the mutation definition or in the harness. It must
    never be recorded as "the candidate tests failed to detect it".
    """


def _docstring_line_span(node):
    """Lines occupied by `node`'s docstring, if it has one."""
    body = getattr(node, "body", None)
    if not body:
        return set()
    first = body[0]
    if not isinstance(first, ast.Expr):
        return set()
    value = first.value
    if not isinstance(value, ast.Constant) or not isinstance(value.value, str):
        return set()
    return set(range(value.lineno, (value.end_lineno or value.lineno) + 1))


def _bare_string_expr_lines(tree):
    """Lines of every bare string expression, not just module/def docstrings.

    A string literal used as a statement anywhere is documentation by
    convention and must never be a mutation target.
    """
    lines = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Expr) and isinstance(node.value, ast.Constant):
            if isinstance(node.value.value, str):
                start = node.lineno
                end = node.end_lineno or start
                lines.update(range(start, end + 1))
    return lines


def documentation_lines(source):
    """Lines occupied by docstrings and by any bare string expression."""
    tree = ast.parse(source)
    lines = _bare_string_expr_lines(tree)
    for node in ast.walk(tree):
        if isinstance(node, (ast.Module, ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            lines |= _docstring_line_span(node)
    return lines


def _bytecode_lines(code, acc):
    """Lines that the compiler actually emitted instructions for."""
    for _offset, lineno in dis.findlinestarts(code):
        if lineno:
            acc.add(lineno)
    for const in code.co_consts:
        if hasattr(const, "co_code"):
            _bytecode_lines(const, acc)
    return acc


def executable_lines(source, filename="<mutant>"):
    """1-based line numbers that carry executable instructions.

    The compiler is the authority, not a text search: a line is executable
    only if instructions were emitted for it. Comment-only and blank lines
    never appear. Documentation is then subtracted, which matters for the
    module docstring -- that one *does* emit a store to `__doc__`, so
    bytecode alone would wrongly call it code.
    """
    emitted = _bytecode_lines(compile(source, filename, "exec"), set())
    return emitted - documentation_lines(source)


def _const_fingerprint(const):
    if hasattr(const, "co_code"):
        return code_fingerprint(const)
    if isinstance(const, tuple):
        return tuple(_const_fingerprint(c) for c in const)
    if isinstance(const, frozenset):
        return ("frozenset", tuple(sorted(repr(c) for c in const)))
    return (type(const).__name__, repr(const))


def code_fingerprint(code):
    """A structural fingerprint of a code object, blind to line numbers.

    Deliberately excludes `co_filename`, `co_firstlineno` and the line table,
    so that adding a comment or a blank line does not look like a change.
    """
    return (
        code.co_name,
        code.co_argcount,
        getattr(code, "co_posonlyargcount", 0),
        code.co_kwonlyargcount,
        code.co_nlocals,
        code.co_flags,
        bytes(code.co_code),
        tuple(code.co_names),
        tuple(code.co_varnames),
        tuple(code.co_freevars),
        tuple(code.co_cellvars),
        tuple(_const_fingerprint(c) for c in code.co_consts),
    )


def _fingerprint_source(source, filename):
    return code_fingerprint(compile(source, filename, "exec"))


def apply_mutation(source, old, new, filename="<mutant>", expect_behaviour_change=True):
    """Return mutated source, having proved the edit landed on real code.

    Raises `VacuousMutation` if the anchor is missing, ambiguous, lands on a
    comment or docstring, or leaves the compiled program unchanged.
    """
    if old == new:
        raise VacuousMutation("mutation is a no-op: old and new text are identical")

    occurrences = source.count(old)
    if occurrences == 0:
        raise VacuousMutation("anchor text not found in source: %r" % (old,))
    if occurrences > 1:
        raise VacuousMutation(
            "anchor text is ambiguous (%d occurrences); make it unique: %r"
            % (occurrences, old)
        )

    index = source.index(old)
    start_line = source.count("\n", 0, index) + 1
    end_line = start_line + old.count("\n")
    touched = set(range(start_line, end_line + 1))

    try:
        runnable = executable_lines(source, filename)
    except SyntaxError as exc:
        raise VacuousMutation("original source does not compile: %s" % (exc,))
    landed_on = sorted(touched & runnable)
    if not landed_on:
        raise VacuousMutation(
            "mutation targets line(s) %s, none of which the compiler emitted "
            "instructions for -- this is the doc-comment trap: the anchor matched "
            "documentation or a comment, not code" % (sorted(touched),)
        )

    mutated = source[:index] + new + source[index + len(old) :]

    try:
        before = _fingerprint_source(source, filename)
    except SyntaxError as exc:
        raise VacuousMutation("original source does not compile: %s" % (exc,))
    try:
        after = _fingerprint_source(mutated, filename)
    except SyntaxError as exc:
        raise VacuousMutation("mutated source does not compile: %s" % (exc,))

    if expect_behaviour_change and before == after:
        raise VacuousMutation(
            "compiled program is byte-identical after the edit -- the mutation "
            "changed nothing executable"
        )

    return mutated
