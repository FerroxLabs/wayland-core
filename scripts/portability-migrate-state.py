#!/usr/bin/env python3
"""portability-migrate-state.py — a normalised fingerprint of a wayland-core home
after a `migrate` apply, for the SC3 interruption proof.

WHY THIS EXISTS, AND WHY IT IS NOT `backup digest`

The interruption proof needs to answer "is the home the migration eventually
produced the SAME home a clean uninterrupted run produces?". A raw tree digest
cannot answer that, because `Provenance::imported_at` is a wall-clock timestamp
written at admit time, so two correct runs of the same migration produce
different trees BY CONSTRUCTION. Comparing raw digests would report every run as
a mismatch, and the natural response to a gate that always fails is to weaken it.

So this normalises exactly ONE field -- `imported_at` -- and NOTHING else. In
particular it does NOT normalise:

  * `config.toml` bytes (the profiles and MCP definitions the apply wrote),
  * the set of quarantined identities,
  * each entry's recorded `digest`, `reason`, `stored_path` and `promote_as`,
  * the bytes of every payload file actually on disk under the store root.

A payload whose bytes drifted, an entry that went missing, an orphan payload
with no index entry, or a profile that failed to land all change the
fingerprint. That is the point: those are precisely the corruptions an
interrupted apply can produce.

Usage:
    portability-migrate-state.py <home>            # one FINGERPRINT: line + facts
    portability-migrate-state.py <home> --verbose  # plus the normalised document

Exit status is 0 whenever the home could be inspected AT ALL, including when the
quarantine index is unparseable -- that is a state the caller must be able to
observe and classify, not an error that aborts the run. The index verdict is
reported as a field (`INDEX: ok|corrupt|absent|empty`) so the caller decides.
"""

import hashlib
import json
import os
import sys

QUARANTINE_DIR = "migrate-quarantine"
QUARANTINE_INDEX = "index.json"
QUARANTINE_PAYLOADS = "payloads"


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def read_index(root):
    """Return (verdict, entries). Verdict is one of absent/empty/corrupt/ok.

    The three non-`ok` verdicts are kept DISTINCT because they are distinct
    product outcomes: `absent` means nothing was ever contained, `empty` means
    the index file exists but reads as no entries (which the product's own
    loader silently treats as a fresh store), and `corrupt` means the file
    exists with content that does not parse. Collapsing them would hide the
    exact failure this proof is looking for.
    """
    path = os.path.join(root, QUARANTINE_DIR, QUARANTINE_INDEX)
    if not os.path.exists(path):
        return "absent", {}
    raw = open(path, "rb").read()
    if not raw.strip():
        return "empty", {}
    try:
        doc = json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError):
        return "corrupt", {}
    entries = doc.get("entries")
    if not isinstance(entries, dict):
        return "corrupt", {}
    return "ok", entries


def normalise_entries(entries):
    """Strip ONLY `provenance.imported_at`; keep every other field."""
    out = {}
    for ident in sorted(entries):
        entry = json.loads(json.dumps(entries[ident]))  # deep copy
        prov = entry.get("provenance")
        if isinstance(prov, dict):
            prov.pop("imported_at", None)
        out[ident] = entry
    return out


def payload_map(root):
    """Every file under the payload root: store-relative path -> sha256.

    Walked from the FILESYSTEM, not from the index, so a payload the index does
    not mention (an orphan left by a kill between the payload write and the
    index save) still appears here and still changes the fingerprint.
    """
    base = os.path.join(root, QUARANTINE_DIR, QUARANTINE_PAYLOADS)
    out = {}
    if not os.path.isdir(base):
        return out
    for dirpath, _dirnames, filenames in os.walk(base):
        for name in filenames:
            abs_path = os.path.join(dirpath, name)
            if os.path.islink(abs_path):
                continue
            rel = os.path.relpath(abs_path, base).replace(os.sep, "/")
            out[rel] = sha256_file(abs_path)
    return out


def config_state(root):
    path = os.path.join(root, "config.toml")
    if not os.path.exists(path):
        return None, 0
    raw = open(path, "rb").read()
    # Profile count read structurally enough to be a non-vacuity check without
    # depending on a TOML parser being installed on the measuring host.
    profiles = raw.decode("utf-8", "replace").count("[profiles.")
    return hashlib.sha256(raw).hexdigest(), profiles


def main():
    if len(sys.argv) < 2:
        print("usage: portability-migrate-state.py <home> [--verbose]", file=sys.stderr)
        return 2
    root = sys.argv[1]
    verbose = "--verbose" in sys.argv[2:]

    verdict, entries = read_index(root)
    normalised = normalise_entries(entries)
    payloads = payload_map(root)
    cfg_digest, profile_count = config_state(root)

    # Orphans: a payload directory on disk that no index entry claims. This is
    # the exact residue a kill between `write_tree` and `save_index` leaves.
    claimed = set()
    for entry in entries.values():
        stored = entry.get("stored_path")
        if isinstance(stored, str):
            claimed.add(stored.split("/")[-1])
    on_disk = {rel.split("/")[0] for rel in payloads}
    orphans = sorted(on_disk - claimed)

    doc = {
        "config_sha256": cfg_digest,
        "quarantine_index_verdict": verdict,
        "quarantine_entries": normalised,
        "payloads": {k: payloads[k] for k in sorted(payloads)},
    }
    blob = json.dumps(doc, sort_keys=True, separators=(",", ":")).encode("utf-8")

    print("FINGERPRINT: " + hashlib.sha256(blob).hexdigest())
    print("INDEX: " + verdict)
    print("ENTRIES: %d" % len(entries))
    print("PAYLOAD-FILES: %d" % len(payloads))
    print("PAYLOAD-DIRS: %d" % len(on_disk))
    print("ORPHAN-PAYLOAD-DIRS: %d" % len(orphans))
    print("CONFIG-PRESENT: " + ("yes" if cfg_digest else "no"))
    print("CONFIG-PROFILES: %d" % profile_count)
    if orphans:
        print("ORPHAN-NAMES: " + ",".join(orphans[:8]))
    if verbose:
        print("---BEGIN-NORMALISED---")
        print(json.dumps(doc, sort_keys=True, indent=2))
        print("---END-NORMALISED---")
    return 0


if __name__ == "__main__":
    sys.exit(main())
