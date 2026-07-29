#!/usr/bin/env python3
"""Extract the trust root the SHIPPED binary bundles, so a release run can verify
its own manifest against it before publishing.

WHY. `manifest-sign` will happily sign with any seed. If the CI signing secret
ever stops corresponding to `RELEASE_TRUST_ROOT_JSON` — a rotation applied on one
side only, a secret pasted from the wrong key file, a key retired in the root —
the release still publishes, and `self-update` then REFUSES every install with
`release manifest signature does not verify`. That failure appears on user
machines, days later, with a green release run behind it. Verifying the signed
manifest in CI against the bundled root turns that into a failed release step.

This is a source extraction because the constant is the authority: the shipped
binary reads that literal and nothing else, so any other copy of the trust root
could drift from it and the check would verify the wrong thing.

Usage:
    extract_bundled_trust_root.py --source crates/wcore-cli/src/update_trust.rs \\
                                  --output trust-root.json
    extract_bundled_trust_root.py --self-test
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
import tempfile

CONST_NAME = "RELEASE_TRUST_ROOT_JSON"
REQUIRED_ROLE = "release_acceptance"

# `pub const RELEASE_TRUST_ROOT_JSON: &str = r#"{...}"#;` — possibly wrapped
# across lines by rustfmt, hence DOTALL and the tolerant whitespace.
PATTERN = re.compile(
    r"pub\s+const\s+" + CONST_NAME + r"\s*:\s*&str\s*=\s*r#\"(?P<json>.*?)\"#\s*;",
    re.DOTALL,
)


class ExtractionError(RuntimeError):
    """Something that must stop the release."""


def extract(source: str) -> dict:
    matches = PATTERN.findall(source)
    if not matches:
        raise ExtractionError(
            f"{CONST_NAME} was not found — the extractor is looking at the wrong file, "
            "or the constant's shape changed and this script must be updated"
        )
    if len(matches) > 1:
        raise ExtractionError(f"{CONST_NAME} is defined {len(matches)} times; refusing to guess")

    try:
        root = json.loads(matches[0])
    except ValueError as error:
        raise ExtractionError(f"{CONST_NAME} is not valid JSON: {error}") from error

    keys = root.get("keys")
    if not isinstance(keys, list) or not keys:
        raise ExtractionError(
            f"{CONST_NAME} carries no keys — the shipped binary would refuse it as a "
            "placeholder, so a manifest signed now could never be verified by it"
        )
    roles = [key.get("role") for key in keys if isinstance(key, dict)]
    if REQUIRED_ROLE not in roles:
        raise ExtractionError(
            f"{CONST_NAME} holds no {REQUIRED_ROLE} key (roles: {roles}); the updater "
            "accepts no other role, so nothing it signs could authorise an install"
        )
    return root


# ---------------------------------------------------------------------------
# Self-test: known-positive, known-negative, and the regression that matters.
# ---------------------------------------------------------------------------

_REAL = (
    'pub const RELEASE_TRUST_ROOT_JSON: &str = r#"{"schema":"wayland.release.trust-root",'
    '"schema_version":1,"keys":[{"key_id":"release-acceptance-key",'
    '"public_key_base64":"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=",'
    '"role":"release_acceptance","valid_from":0,"retired_at":null}]}"#;'
)
_PLACEHOLDER = (
    "pub const RELEASE_TRUST_ROOT_JSON: &str =\n"
    '    r#"{"schema":"wayland.release.trust-root","schema_version":1,"keys":[]}"#;'
)
_WRONG_ROLE = _REAL.replace("release_acceptance", "packaging")


def self_test() -> int:
    failures: list[str] = []

    def check(label: str, condition: bool) -> None:
        print(f"{'PASS' if condition else 'FAIL'}  {label}")
        if not condition:
            failures.append(label)

    def refuses(source: str) -> bool:
        try:
            extract(source)
            return False
        except ExtractionError:
            return True

    # 1. KNOWN-POSITIVE, surrounded by unrelated source so the match is not
    #    trivially the whole input.
    padded = "//! docs\nuse std::fmt;\n" + _REAL + "\nconst OTHER: u32 = 1;\n"
    root = extract(padded)
    check("extracts one key from a real constant", len(root["keys"]) == 1)
    check("extracts the acceptance role", root["keys"][0]["role"] == REQUIRED_ROLE)

    # 2. KNOWN-NEGATIVE — a file that simply does not hold the constant.
    check("refuses a source without the constant", refuses("fn main() {}\n"))
    check("refuses a source with two definitions", refuses(_REAL + "\n" + _REAL))
    check("refuses a malformed literal", refuses('pub const RELEASE_TRUST_ROOT_JSON: &str = r#"{"#;'))

    # 3. THE ASSERTION THAT PROVES THIS SCRIPT EARNS ITS PLACE. A regex that
    #    merely FINDS the constant would happily hand CI the empty placeholder,
    #    and the release would publish a manifest the shipped binary refuses.
    #    Both degenerate roots must stop the run.
    check("refuses the empty placeholder root", refuses(_PLACEHOLDER))
    check("refuses a root with no acceptance-role key", refuses(_WRONG_ROLE))

    # And it must survive rustfmt wrapping the literal onto its own line.
    check("handles a line-wrapped constant", refuses(_PLACEHOLDER) and not refuses(_REAL))

    # End to end through the real file interface.
    with tempfile.TemporaryDirectory() as raw:
        path = pathlib.Path(raw) / "update_trust.rs"
        path.write_text(padded, encoding="utf-8")
        check("reads from a file", extract(path.read_text(encoding="utf-8"))["schema_version"] == 1)

    print(f"\n{len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=pathlib.Path)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()

    if arguments.self_test:
        return self_test()
    if arguments.source is None or arguments.output is None:
        parser.error("--source and --output are required unless --self-test is given")

    try:
        root = extract(arguments.source.read_text(encoding="utf-8"))
    except (OSError, ExtractionError) as error:
        print(f"REFUSING TO EXTRACT A TRUST ROOT: {error}", file=sys.stderr)
        return 1

    arguments.output.write_text(json.dumps(root, indent=2), encoding="utf-8")
    ids = ", ".join(f"{key['key_id']}({key['role']})" for key in root["keys"])
    print(f"bundled trust root: {len(root['keys'])} key(s) — {ids}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
