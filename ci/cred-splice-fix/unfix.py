#!/usr/bin/env python3
"""RED CONTROL for the cred-splice-fix lane. TEMPORARY — DELETE AT MERGE.

Reverts the two defects in `chunked_put` in place so CI can show the new proofs
FAILING before it shows them passing. A test suite that has only ever been run
against the fixed code proves nothing about the defect it claims to pin, and a
red control that cannot go red is worth no more than a gate that cannot fail.

Three edits, each the exact shape of the shipped defect:

1. Drop the cross-process write lock from `chunked_put`, restoring the
   unsynchronised read-modify-write on the manifest.
2. Drop the "writer B made no progress" assertion from the pinned race test, so
   the run reaches the final assertion and PRINTS the spliced value
   (`len=9000 tags=[BA]`) instead of stopping at the earlier symptom.
3. Restore `raw.get(key).ok().flatten()` in `read_previous_manifest`, so a read
   fault is once again indistinguishable from "no previous manifest".
"""

import re
import sys

PATH = "crates/wcore-config/src/credentials.rs"


def main() -> int:
    source = open(PATH, encoding="utf-8").read()

    lock = """    let _lock = locks.acquire(key)?;
    let previous = read_previous_manifest(raw, key)?;

    if utf16_units(value) <= max_units {"""
    if source.count(lock) != 1:
        print("FAIL: the write lock is not where this control expects it")
        return 2
    source = source.replace(
        lock,
        """    let previous = read_previous_manifest(raw, key)?;

    if utf16_units(value) <= max_units {""",
        1,
    )
    source = source.replace(
        """    max_units: usize,
    locks: &ChunkWriteLockSite,
) -> Result<(), CredentialsError> {
    let previous""",
        """    max_units: usize,
    _locks: &ChunkWriteLockSite,
) -> Result<(), CredentialsError> {
    let previous""",
        1,
    )

    blocked = re.search(
        r"\n        assert_eq!\(\n"
        r"            chunked_get\(&Scheduled::plain\(&shared\), KEY, &locks\)\n"
        r"                \.unwrap\(\)\n"
        r"                \.as_deref\(\),\n"
        r"            Some\(seeded\.as_str\(\)\),\n"
        r"            \"while writer A is inside.*?\n        \);\n",
        source,
        re.S,
    )
    if not blocked:
        print("FAIL: the blocked-writer assertion is not where this control expects it")
        return 2
    source = source[: blocked.start()] + "\n" + source[blocked.end() :]
    source = source.replace(
        "        let seeded = ", "        #[allow(unused)]\n        let seeded = ", 1
    )

    swallow = "    Ok(raw.get(key)?.as_deref().and_then(parse_chunk_manifest))"
    if source.count(swallow) != 1:
        print("FAIL: the primary-read abort is not where this control expects it")
        return 2
    source = source.replace(
        swallow,
        """    Ok(raw
        .get(key)
        .ok()
        .flatten()
        .as_deref()
        .and_then(parse_chunk_manifest))""",
        1,
    )

    open(PATH, "w", encoding="utf-8").write(source)
    print("RED CONTROL APPLIED: lock removed, primary-read fault swallowed again")
    return 0


if __name__ == "__main__":
    sys.exit(main())
