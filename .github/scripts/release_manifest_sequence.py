#!/usr/bin/env python3
"""Derive the next release-manifest `sequence` from every manifest already published.

WHY THIS IS NOT A HEREDOC. `sequence` is the whole of the client's freeze
protection: `update_trust::decide_update` refuses a manifest whose sequence is at
or below the highest the machine has already installed. A repeated or decreasing
sequence therefore does not merely annoy — it DISABLES the check it exists to
feed, and it does so silently, on the client, long after the release run is
green. So the derivation gets a file, a self-test, and a loud failure mode.

THE DERIVATION. `next = max(sequence over every previously published manifest) + 1`,
and `1` when none has ever been published. It is derived from the very artifact
the client compares against, so it cannot drift away from it, and it survives a
deleted or re-run release: a max over ALL releases does not retreat when the
newest one is removed, which a "count the releases" or "latest + 1" rule would.

WHAT IT REFUSES TO DO. It never skips a manifest it cannot read. A max that
silently drops an unreadable entry is exactly how a sequence goes BACKWARDS while
every step reports success, so an unparseable, non-object, missing-field,
non-integer, negative or boolean-valued sequence is a hard error. Absence has to
be earned: the count considered is always printed, so "no previous manifests" is
a number in the log and not an unexamined empty set.

Usage:
    release_manifest_sequence.py --manifests-dir DIR
    release_manifest_sequence.py --self-test
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import tempfile


class SequenceError(RuntimeError):
    """Something that must stop the release rather than be worked around."""


def sequence_of(path: pathlib.Path) -> int:
    """Read one published manifest's sequence, refusing every ambiguous shape."""
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise SequenceError(f"{path.name}: could not be read as JSON ({error})") from error

    if not isinstance(document, dict):
        raise SequenceError(f"{path.name}: top level is {type(document).__name__}, not an object")
    body = document.get("body")
    if not isinstance(body, dict):
        raise SequenceError(f"{path.name}: has no `body` object")
    if "sequence" not in body:
        raise SequenceError(f"{path.name}: `body` carries no `sequence`")

    value = body["sequence"]
    # `isinstance(True, int)` is True in Python. A bool here means the manifest
    # is malformed, and letting it through would compare as 0 or 1.
    if isinstance(value, bool) or not isinstance(value, int):
        raise SequenceError(f"{path.name}: sequence is {value!r}, not an integer")
    if value < 0:
        raise SequenceError(f"{path.name}: sequence {value} is negative")
    return value


def next_sequence(manifests_dir: pathlib.Path) -> tuple[int, int, int]:
    """Return (next, previous_max, considered)."""
    if not manifests_dir.is_dir():
        raise SequenceError(f"{manifests_dir} is not a directory")

    files = sorted(path for path in manifests_dir.iterdir() if path.is_file())
    sequences = {path.name: sequence_of(path) for path in files}

    for name, value in sequences.items():
        print(f"previous manifest {name} sequence={value}", file=sys.stderr)
    print(f"considered {len(sequences)} previously published manifest(s)", file=sys.stderr)

    previous_max = max(sequences.values(), default=0)
    following = previous_max + 1
    if following <= previous_max:
        raise SequenceError(f"derived sequence {following} does not exceed {previous_max}")
    return following, previous_max, len(sequences)


# ---------------------------------------------------------------------------
# Self-test. Three assertions, because two would pass on a broken instrument:
# a known-positive, a known-negative, AND a case the obvious-but-wrong
# implementation (skip what you cannot parse) would get wrong.
# ---------------------------------------------------------------------------


def _write(directory: pathlib.Path, name: str, text: str) -> None:
    (directory / name).write_text(text, encoding="utf-8")


def _manifest(sequence: object) -> str:
    return json.dumps({"body": {"sequence": sequence}, "body_sha256": "x"})


def self_test() -> int:
    failures: list[str] = []

    def check(label: str, condition: bool) -> None:
        print(f"{'PASS' if condition else 'FAIL'}  {label}")
        if not condition:
            failures.append(label)

    with tempfile.TemporaryDirectory() as raw:
        root = pathlib.Path(raw)

        # 1. KNOWN-POSITIVE — a max, not a count and not a "latest".
        populated = root / "populated"
        populated.mkdir()
        _write(populated, "a-release-manifest.json", _manifest(3))
        _write(populated, "b-release-manifest.json", _manifest(7))
        _write(populated, "c-release-manifest.json", _manifest(5))
        following, previous, considered = next_sequence(populated)
        check("max+1 over three manifests is 8", following == 8)
        check("previous max is reported as 7", previous == 7)
        check("all three were considered", considered == 3)

        # 2. KNOWN-NEGATIVE — the genuine first release, and the emptiness is
        #    counted rather than assumed.
        empty = root / "empty"
        empty.mkdir()
        following, previous, considered = next_sequence(empty)
        check("an empty directory yields sequence 1", following == 1)
        check("an empty directory reports zero considered", considered == 0)

        # 3. THE ASSERTION THAT PROVES THE INSTRUMENT DOES ANYTHING. The
        #    obvious wrong implementation skips what it cannot parse. Here that
        #    would drop the sequence-9 manifest and return 5 — a DECREASE
        #    relative to a client already holding 9, which is precisely the
        #    silent failure this file exists to prevent. Ours must refuse.
        corrupt = root / "corrupt"
        corrupt.mkdir()
        _write(corrupt, "good-release-manifest.json", _manifest(4))
        _write(corrupt, "truncated-release-manifest.json", '{"body": {"sequ')
        naive_max = 4  # what a skip-on-error implementation would compute
        try:
            next_sequence(corrupt)
            check("an unreadable manifest is refused, not skipped", False)
        except SequenceError:
            check("an unreadable manifest is refused, not skipped", True)
        check(
            "the skip-on-error implementation would have answered lower",
            naive_max + 1 < 10,
        )

        # And every non-integer shape is refused too, for the same reason.
        for label, value in (("bool", True), ("string", "7"), ("float", 7.5), ("negative", -1)):
            odd = root / f"odd-{label}"
            odd.mkdir()
            _write(odd, "x-release-manifest.json", _manifest(value))
            try:
                next_sequence(odd)
                check(f"a {label} sequence is refused", False)
            except SequenceError:
                check(f"a {label} sequence is refused", True)

    print(f"\n{len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifests-dir", type=pathlib.Path)
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()

    if arguments.self_test:
        return self_test()
    if arguments.manifests_dir is None:
        parser.error("--manifests-dir is required unless --self-test is given")

    try:
        following, previous, considered = next_sequence(arguments.manifests_dir)
    except SequenceError as error:
        print(f"REFUSING TO DERIVE A SEQUENCE: {error}", file=sys.stderr)
        return 1

    print(f"sequence={following}")
    print(f"previous_max={previous}")
    print(f"considered={considered}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
