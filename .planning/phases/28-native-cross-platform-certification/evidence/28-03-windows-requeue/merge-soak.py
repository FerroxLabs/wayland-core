#!/usr/bin/env python3
"""Fold the measured windows family into the canonical 28-03 soak record.

The record 28-03 wrote carries linux and macos under `families`, and windows under
`families_not_run` with a host-access reason that has since been shown false. This replaces
that placeholder with the MEASURED windows record.

It refuses to do so unless the windows record actually stands up, because a merge that
accepts anything would turn `--check-session-count` green by construction and the gate would
stop meaning anything:

  * the family must be `windows`
  * it must have completed the full session target
  * its binary digest must equal the ledger digest it recorded
  * `families_not_run` may only be emptied of the windows row -- any other not-run family
    stays, so this cannot be used to launder a different missing family

Usage: merge-soak.py <canonical soak.json> <windows-soak.json> <out soak.json>
"""
import json
import sys

SESSION_TARGET = 1000


def main() -> int:
    canon_p, win_p, out_p = sys.argv[1], sys.argv[2], sys.argv[3]
    canon = json.load(open(canon_p))
    win = json.load(open(win_p))

    fam = win.get("family")
    if fam != "windows":
        print(f"REFUSED: record is family {fam!r}, not 'windows'")
        return 2
    done = win.get("sessions_completed")
    if done != SESSION_TARGET:
        print(f"REFUSED: windows completed {done}/{SESSION_TARGET}; a short run is not a run")
        return 3
    b, l = win.get("binary_sha256"), win.get("ledger_sha256")
    if not b or not l or b != l:
        print(f"REFUSED: windows binary {b!r} is not the ledger-bound {l!r}")
        return 4

    already = [f.get("family") for f in canon.get("families", [])]
    if "windows" in already:
        print("REFUSED: canonical record already carries a windows family")
        return 5

    canon.setdefault("families", []).append(win)
    kept = [f for f in (canon.get("families_not_run") or []) if f.get("family") != "windows"]
    canon["families_not_run"] = kept

    with open(out_p, "w") as fh:
        json.dump(canon, fh, indent=2)
        fh.write("\n")
    print(f"MERGED windows into {out_p}; families={[f.get('family') for f in canon['families']]}; "
          f"families_not_run={[f.get('family') for f in kept]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
