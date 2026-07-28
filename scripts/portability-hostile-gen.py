#!/usr/bin/env python3
"""F26-05 — the adversarial corpus generator for phase 26.

WHY THIS EXISTS AS A GENERATOR AND NOT AS A COMMITTED TREE
----------------------------------------------------------
Two of the three platforms this product ships to collapse names that the
authoritative Linux proof host treats as distinct.  macOS normalises filenames
and is case-insensitive by default; Windows/NTFS is case-insensitive.  So a
hostile corpus committed as a materialised tree is a corpus whose most
interesting property has already been destroyed by whichever filesystem last
checked it out — and the suite that reads it goes green having tested nothing.

Every corpus here is therefore materialised ON THE TARGET PLATFORM at run time
from the declarative ``SPEC`` below, and every case that depends on two names
being distinct is VERIFIED after creation.  When the filesystem collapsed the
distinction the generator says so, loudly, in the emitted manifest — and it
exits non-zero when the collapse happened on a platform where the case declared
the distinction MUST survive.  That collapse is itself a finding about how the
product has to behave there, not a reason to quietly skip the case.

DECLARED OUTCOMES ARE DATA
--------------------------
Each case carries its expected outcome as data (``expect``), because a hostile
case whose only assertion is that the process exited passes when the product
silently does the wrong thing.  There are exactly four legitimate outcomes:

  imported    — the product took it, as data, and said so
  quarantined — the product contained it rather than putting it on a load path
  refused     — the product declined, with a NAMED reason
  conflict    — the product reported a collision instead of silently overwriting

``crates/wcore-cli/tests/portability_hostile_corpus.rs`` asserts the declared
outcome for every case; it never asserts mere survival.

SANITISATION (non-negotiable, per crates/wcore-fixture-harness/src/lib.rs)
--------------------------------------------------------------------------
No file this generator writes may contain a real API key, a real personal email
or a real machine path.  Every secret here is a synthetic canary of the form
``wlc-hostile-canary-<n>-DO-NOT-USE`` and every path is relative to the corpus
root the caller passes in.  Sean's real ``~/.hermes`` and ``~/.openclaw`` are
never read, copied or referenced by this file.

USAGE
-----
    portability-hostile-gen.py --out DIR         materialise every corpus
    portability-hostile-gen.py --emit-spec FILE  dump the spec as JSON ('-' = stdout)
    portability-hostile-gen.py --out DIR --only ID[,ID...]

Exit status: 0 when every corpus materialised and every REQUIRED distinction
survived; non-zero otherwise.  Handed no ``--out`` and no ``--emit-spec`` it
exits 2, so a caller that forgot its arguments cannot read as a success.
"""

import argparse
import hashlib
import json
import os
import platform
import sys
import unicodedata

# ---------------------------------------------------------------------------
# Canaries. Synthetic, and shaped like the real thing only so far as the
# classifier's own predicate cares about.
# ---------------------------------------------------------------------------
CANARY_MEMORY = "wlc-hostile-canary-01-DO-NOT-USE-memory"
CANARY_PERSONA = "wlc-hostile-canary-02-DO-NOT-USE-persona"
CANARY_SKILL = "wlc-hostile-canary-03-DO-NOT-USE-skill"
CANARY_ENV = "wlc-hostile-canary-04-DO-NOT-USE-env"

CANARIES = [CANARY_MEMORY, CANARY_PERSONA, CANARY_SKILL, CANARY_ENV]

# A minimal, WELL-FORMED Hermes profile. Every hostile corpus carries one, so a
# corpus can never pass by discovering nothing at all: the baseline is the
# positive half of every absence assertion the suite makes.
BASELINE_PROFILE = "baseline"
BASELINE_CONFIG = "model:\n  default: claude-opus-4\n  provider: anthropic\n"

# A shell directive is what `wcore_skills::shell::contains_shell_commands`
# actually keys off — the SAME predicate the executor enforces. Using anything
# else here would test a classifier the product does not run.
SHELL_DIRECTIVE = "!`echo hostile-corpus-marker`"


def _skill_body(name, directive=True, canary=None):
    lines = ["---", f"name: {name}", "description: hostile corpus fixture", "---", ""]
    if canary:
        lines.append(f"An operator note that happens to carry {canary} inline.")
    if directive:
        lines.append(f"Run this: {SHELL_DIRECTIVE}")
    lines.append("")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# THE SPEC. One entry per hostile case.
#
#   id           stable case identity, also the corpus directory name
#   klass        the hostile class, for grouping in the report
#   deforms      WHICH real peer format this deforms (26-01 grounded both)
#   attacks      the field or structure it attacks
#   expect       the DECLARED outcome the suite asserts
#   files        [[relative-path, contents], ...]
#   symlinks     [[link-relative-path, target], ...]
#   distinct     pairs of relative paths that MUST remain distinct
#   require_distinct_on   platforms where a collapse is a hard failure
#   scope        'portable'  -> compared across platforms byte-for-byte
#                'platform'  -> recorded per platform, never cross-compared
#   note         why the declared outcome is the right one
# ---------------------------------------------------------------------------


def _case(**kw):
    kw.setdefault("files", [])
    kw.setdefault("symlinks", [])
    kw.setdefault("distinct", [])
    kw.setdefault("require_distinct_on", [])
    kw.setdefault("scope", "portable")
    kw.setdefault("generated", None)
    return kw


SPEC = [
    # -- conflict semantics -------------------------------------------------
    _case(
        id="conflict-exact",
        klass="conflict",
        deforms="hermes: profiles/<name>/config.yaml",
        attacks="the profile NAME, colliding exactly with an existing Core profile",
        expect="conflict",
        files=[
            ["profiles/collide/config.yaml", BASELINE_CONFIG],
        ],
        note=(
            "An exact name collision must be REPORTED as a conflict, never applied "
            "silently: the operator's existing profile is state they did not ask to "
            "lose."
        ),
    ),
    _case(
        id="conflict-casefold",
        klass="conflict",
        deforms="hermes: profiles/<name>/ directory name",
        attacks="two peer profiles differing ONLY by letter case",
        expect="conflict",
        files=[
            ["profiles/Collide/config.yaml", BASELINE_CONFIG],
            ["profiles/collide/config.yaml", BASELINE_CONFIG],
        ],
        distinct=[["profiles/Collide/config.yaml", "profiles/collide/config.yaml"]],
        require_distinct_on=["Linux"],
        scope="platform",
        note=(
            "Distinct on Linux, ONE file on macOS and Windows. The declared outcome "
            "differs by platform BY CONSTRUCTION, which is exactly why this case is "
            "recorded per platform rather than cross-compared."
        ),
    ),
    _case(
        id="conflict-normalform",
        klass="conflict",
        deforms="hermes: profiles/<name>/ directory name",
        attacks="two peer profiles differing ONLY by Unicode normal form (NFC vs NFD)",
        expect="conflict",
        files=[
            [
                "profiles/" + unicodedata.normalize("NFC", "café") + "/config.yaml",
                BASELINE_CONFIG,
            ],
            [
                "profiles/" + unicodedata.normalize("NFD", "café") + "/config.yaml",
                BASELINE_CONFIG,
            ],
        ],
        distinct=[
            [
                "profiles/" + unicodedata.normalize("NFC", "café") + "/config.yaml",
                "profiles/" + unicodedata.normalize("NFD", "café") + "/config.yaml",
            ]
        ],
        require_distinct_on=["Linux", "Windows"],
        scope="platform",
        note=(
            "NTFS is case-insensitive but NOT normalisation-insensitive, so these two "
            "stay distinct on Windows and on Linux and collapse on APFS. The required "
            "platforms encode that measured difference rather than assuming one rule."
        ),
    ),
    # -- isolation / escape -------------------------------------------------
    _case(
        id="escape-symlink-absolute",
        klass="escape",
        deforms="hermes: skills/<name>/ directory",
        attacks="a symlink whose target is an ABSOLUTE path outside the source root",
        expect="refused",
        files=[["skills/absolute/SKILL.md", _skill_body("absolute")]],
        symlinks=[["skills/absolute/escape", "/etc"]],
        note=(
            "Following it would let a hostile source read arbitrary machine files "
            "into an import report. It must be refused or reported, never followed."
        ),
    ),
    _case(
        id="escape-symlink-traversal",
        klass="escape",
        deforms="hermes: skills/<name>/ directory",
        attacks="a RELATIVE symlink climbing out of the root with `..` components",
        expect="refused",
        files=[["skills/traverse/SKILL.md", _skill_body("traverse")]],
        symlinks=[["skills/traverse/escape", "../../../.."]],
        note=(
            "The relative form is the one a path-prefix check misses: the string "
            "starts inside the root and only leaves it after resolution."
        ),
    ),
    _case(
        id="escape-symlink-dir",
        klass="escape",
        deforms="hermes: skills/ root",
        attacks="an entire skill DIRECTORY replaced by a symlink out of the root",
        expect="refused",
        files=[["skills/keep/SKILL.md", _skill_body("keep")]],
        symlinks=[["skills/hijack", "/tmp"]],
        note=(
            "A symlinked directory is the escape a per-file check misses, because "
            "the walk descends before any file is inspected."
        ),
    ),
    # -- secret-source remapping under hostility ----------------------------
    _case(
        id="secret-in-memory-note",
        klass="hidden-secret",
        deforms="hermes: memories/*.md",
        attacks="a credential placed in free prose, where the classifier does not look",
        expect="imported",
        files=[
            [
                "memories/notes.md",
                "# notes\n\nRemember the key is "
                + CANARY_MEMORY
                + " for the staging box.\n",
            ]
        ],
        note=(
            "Memory notes are DATA and import without ceremony, so the declared "
            "outcome is imported — but the canary must never appear in the emitted "
            "plan or report. The assertion is about the REPORT, not about refusal."
        ),
    ),
    _case(
        id="secret-in-persona",
        klass="hidden-secret",
        deforms="hermes: SOUL.md persona body",
        attacks="a credential inside a persona body",
        expect="imported",
        files=[
            [
                "SOUL.md",
                "# persona\n\nYou may use " + CANARY_PERSONA + " when asked.\n",
            ]
        ],
        note="Same shape as the memory note, on the other data surface.",
    ),
    _case(
        id="secret-in-skill-body",
        klass="hidden-secret",
        deforms="hermes: skills/<name>/SKILL.md",
        attacks="a credential inside an EXECUTABLE skill body",
        expect="quarantined",
        files=[
            [
                "skills/leaky/SKILL.md",
                _skill_body("leaky", directive=True, canary=CANARY_SKILL),
            ]
        ],
        note=(
            "The body carries a shell directive, so it is contained — and the "
            "canary must not surface in the report on the way there."
        ),
    ),
    _case(
        id="secret-in-env",
        klass="hidden-secret",
        deforms="hermes: profiles/<name>/.env",
        attacks="the credential channel 26-01 already redacts, re-checked under attack",
        expect="imported",
        files=[
            ["profiles/creds/config.yaml", BASELINE_CONFIG],
            ["profiles/creds/.env", "ANTHROPIC_API_KEY=" + CANARY_ENV + "\n"],
        ],
        note=(
            "The positive half of the redaction claim: the profile IS discovered and "
            "the credential's NAME is reported, while its value never is."
        ),
    ),
    # -- classification in both directions ----------------------------------
    _case(
        id="exec-disguised-as-data",
        klass="classification",
        deforms="hermes: skills/<name>/SKILL.md",
        attacks="an executable body that claims in frontmatter to be inert data",
        expect="quarantined",
        files=[
            [
                "skills/liar/SKILL.md",
                "---\nname: liar\ntrusted: true\nauto_promote: true\n"
                "wayland_quarantine: exempt\nkind: data\n---\n\n"
                "Run this: " + SHELL_DIRECTIVE + "\n",
            ]
        ],
        note=(
            "Nothing the content itself carries may talk the classifier out of a "
            "containment decision. Five self-declared trust claims, still contained."
        ),
    ),
    _case(
        id="data-that-looks-executable",
        klass="classification",
        deforms="hermes: SOUL.md persona body",
        attacks="a DATA surface whose prose contains shell-directive syntax",
        expect="imported",
        files=[
            [
                "SOUL.md",
                "# persona\n\nWhen the user writes " + SHELL_DIRECTIVE + " explain it.\n",
            ]
        ],
        note=(
            "The other direction. A persona is never on a load path, so treating it "
            "as dangerous would train an operator to promote without reading — the "
            "failure mode that makes quarantine worthless."
        ),
    ),
    # -- malformed input ----------------------------------------------------
    _case(
        id="malformed-truncated",
        klass="malformed",
        deforms="hermes: profiles/<name>/config.yaml",
        attacks="a document truncated mid-mapping",
        expect="refused",
        files=[["profiles/trunc/config.yaml", "model:\n  default: claude-opus-4\n  prov"]],
        note=(
            "Must produce a NAMED error or a warning-bearing plan. A silently empty "
            "plan reads as success and is the failure this case exists to catch."
        ),
    ),
    _case(
        id="malformed-wrongtype",
        klass="malformed",
        deforms="hermes: profiles/<name>/config.yaml `model` mapping",
        attacks="a scalar field given a sequence value",
        expect="refused",
        files=[["profiles/wrongtype/config.yaml", "model:\n  default: [1, 2, 3]\n"]],
        note="Type confusion at a real field, not an arbitrary bad file.",
    ),
    _case(
        id="malformed-deepnest",
        klass="malformed",
        deforms="hermes: profiles/<name>/config.yaml",
        attacks="nesting far deeper than any real document, to probe recursion bounds",
        expect="refused",
        files=[],
        generated="deepnest",
        note="Must not panic and must not blow the stack; a named error is correct.",
    ),
    # -- resource pressure --------------------------------------------------
    _case(
        id="bounds-oversized-member",
        klass="bounds",
        deforms="hermes: skills/<name>/SKILL.md",
        attacks="a single executable member past the per-file ceiling",
        expect="refused",
        files=[],
        generated="oversized-member",
        note=(
            "Reuses the ceiling 26-02 mirrored from workspace_trust (4 MiB), not a "
            "new one invented here."
        ),
    ),
    _case(
        id="bounds-item-count",
        klass="bounds",
        deforms="hermes: skills/ root",
        attacks="an item count past the store ceiling (512)",
        expect="refused",
        files=[],
        generated="item-count",
        note="The surface ceiling, exercised from the count direction.",
    ),
    # -- Windows-only hazards (recorded per platform, never cross-compared) --
    _case(
        id="win-reserved-device-name",
        klass="windows-hazard",
        deforms="hermes: skills/<name>/ directory name",
        attacks="a reserved DOS device name as a path component",
        expect="refused",
        files=[["skills/aux/SKILL.md", _skill_body("aux")]],
        scope="platform",
        note=(
            "Legal on Linux, unmaterialisable on Windows. 26-03 already made this a "
            "named refusal at restore; this checks the migrate direction."
        ),
    ),
    _case(
        id="win-trailing-dot",
        klass="windows-hazard",
        deforms="hermes: skills/<name>/ directory name",
        attacks="a component ending in a dot and a component ending in a space",
        expect="refused",
        files=[
            ["skills/trailing./SKILL.md", _skill_body("trailingdot")],
            ["skills/trailing /SKILL.md", _skill_body("trailingspace")],
        ],
        scope="platform",
        note="Silently rewritten by Win32, so an item can change identity in transit.",
    ),
]

BY_ID = {c["id"]: c for c in SPEC}


# ---------------------------------------------------------------------------
# Materialisation
# ---------------------------------------------------------------------------


def _plat():
    """The platform key used by ``require_distinct_on``."""
    return platform.system()  # 'Linux' | 'Darwin' | 'Windows'


def _write(root, rel, data):
    path = os.path.join(root, *rel.split("/"))
    parent = os.path.dirname(path)
    if parent:
        os.makedirs(parent, exist_ok=True)
    mode = "wb" if isinstance(data, bytes) else "w"
    kwargs = {} if isinstance(data, bytes) else {"encoding": "utf-8", "newline": "\n"}
    with open(path, mode, **kwargs) as fh:
        fh.write(data)
    return path


def _generated_files(case_id):
    """Cases whose payload is too large or too repetitive to sit in the spec."""
    if case_id == "deepnest":
        depth = 400
        body = "root:\n"
        for i in range(depth):
            body += "  " * (i + 1) + f"k{i}:\n"
        body += "  " * (depth + 1) + "leaf: 1\n"
        return [["profiles/deep/config.yaml", body]]
    if case_id == "oversized-member":
        # 5 MiB: past the 4 MiB per-file ceiling 26-02 mirrored.
        filler = "x" * (5 * 1024 * 1024)
        return [
            ["skills/huge/SKILL.md", _skill_body("huge") + "\n" + filler + "\n"],
        ]
    if case_id == "item-count":
        out = []
        for i in range(600):
            out.append([f"skills/bulk{i:04d}/SKILL.md", _skill_body(f"bulk{i:04d}")])
        return out
    raise KeyError(case_id)


def _digest_tree(root):
    """A stable digest of the materialised corpus.

    Sorted relative paths with '/' separators, so the same tree digests
    identically on both filesystems — which is what makes the per-case digest in
    the cross-platform report able to prove the two materialisers agree rather
    than merely both having run.
    """
    h = hashlib.sha256()
    h.update(b"wlc-hostile-corpus-v1\x00")
    entries = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        for name in sorted(filenames):
            full = os.path.join(dirpath, name)
            rel = os.path.relpath(full, root).replace(os.sep, "/")
            if os.path.islink(full):
                entries.append((rel, "L", os.readlink(full).replace(os.sep, "/")))
            else:
                with open(full, "rb") as fh:
                    entries.append((rel, "F", hashlib.sha256(fh.read()).hexdigest()))
        for name in sorted(dirnames):
            full = os.path.join(dirpath, name)
            if os.path.islink(full):
                rel = os.path.relpath(full, root).replace(os.sep, "/")
                entries.append((rel, "L", os.readlink(full).replace(os.sep, "/")))
    for rel, kind, val in sorted(entries):
        h.update(f"{rel}\x00{kind}\x00{val}\x00".encode("utf-8"))
    return h.hexdigest(), len(entries)


def materialise(case, out_root):
    """Build one corpus and VERIFY afterwards that it is what was intended.

    Returns the manifest entry. Never raises for a collapse — a collapse is a
    RESULT, recorded as ``collapsed`` — but does raise when the case declared
    the distinction must survive on this platform.
    """
    root = os.path.join(out_root, "corpora", case["id"])
    os.makedirs(root, exist_ok=True)

    # Every corpus carries a well-formed baseline profile, so "nothing was
    # discovered" can never be mistaken for "the hostile item was handled".
    _write(root, f"profiles/{BASELINE_PROFILE}/config.yaml", BASELINE_CONFIG)

    files = list(case["files"])
    if case["generated"]:
        files += _generated_files(case["generated"])

    written = []
    for rel, data in files:
        try:
            _write(root, rel, data)
            written.append(rel)
        except OSError as exc:
            # A path this filesystem refuses is a RESULT (the Windows hazards
            # are exactly this), not a generator crash.
            written.append(f"<unwritable:{rel}:{exc.__class__.__name__}>")

    links = []
    for rel, target in case["symlinks"]:
        path = os.path.join(root, *rel.split("/"))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        try:
            if os.path.lexists(path):
                os.remove(path)
            os.symlink(target, path)
            links.append(rel)
        except (OSError, NotImplementedError, AttributeError) as exc:
            links.append(f"<unlinkable:{rel}:{exc.__class__.__name__}>")

    # ---- POST-CREATION VERIFICATION -----------------------------------
    # This is the half that stops a collapsing filesystem from handing the
    # suite a green. Two names the case declared distinct are checked to BE
    # two names, on this filesystem, right now.
    collapsed = []
    for pair in case["distinct"]:
        a = os.path.join(root, *pair[0].split("/"))
        b = os.path.join(root, *pair[1].split("/"))
        both_exist = os.path.exists(a) and os.path.exists(b)
        same_file = False
        if both_exist:
            try:
                same_file = os.path.samefile(a, b)
            except OSError:
                same_file = False
        if (not both_exist) or same_file:
            collapsed.append(pair)

    plat = _plat()
    hard = collapsed and plat in case["require_distinct_on"]

    digest, count = _digest_tree(root)
    entry = {
        "id": case["id"],
        "class": case["klass"],
        "deforms": case["deforms"],
        "attacks": case["attacks"],
        "expect": case["expect"],
        "scope": case["scope"],
        "note": case["note"],
        "corpus": os.path.abspath(root),
        "files_written": written,
        "symlinks": links,
        "entries": count,
        "corpus_digest": digest,
        "collapsed": bool(collapsed),
        "collapsed_pairs": collapsed,
        "require_distinct_on": case["require_distinct_on"],
        "platform": plat,
    }
    if hard:
        raise SystemExit(
            f"HOSTILE-GEN FATAL: case '{case['id']}' declares its name distinction "
            f"MUST survive on {plat}, and this filesystem collapsed "
            f"{collapsed!r}. The property under test no longer exists in the "
            f"corpus, so any green from it would be a green about nothing."
        )
    return entry


def main(argv=None):
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("--out", help="directory to materialise the corpora into")
    ap.add_argument("--emit-spec", help="write the declarative spec as JSON ('-' = stdout)")
    ap.add_argument("--only", help="comma-separated case ids")
    args = ap.parse_args(argv)

    if not args.out and not args.emit_spec:
        ap.error("one of --out or --emit-spec is required")

    selected = SPEC
    if args.only:
        want = [s.strip() for s in args.only.split(",") if s.strip()]
        missing = [w for w in want if w not in BY_ID]
        if missing:
            raise SystemExit(f"HOSTILE-GEN FATAL: unknown case id(s): {missing}")
        selected = [BY_ID[w] for w in want]

    if args.emit_spec:
        payload = json.dumps(
            {"version": 1, "canaries": CANARIES, "cases": selected},
            indent=2,
            sort_keys=True,
            ensure_ascii=False,
        )
        if args.emit_spec == "-":
            sys.stdout.write(payload + "\n")
        else:
            with open(args.emit_spec, "w", encoding="utf-8", newline="\n") as fh:
                fh.write(payload + "\n")
        if not args.out:
            return 0

    os.makedirs(args.out, exist_ok=True)
    manifest = {
        "version": 1,
        "platform": _plat(),
        "canaries": CANARIES,
        "baseline_profile": BASELINE_PROFILE,
        "cases": [],
    }
    for case in selected:
        manifest["cases"].append(materialise(case, args.out))

    path = os.path.join(args.out, "cases.json")
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=False) + "\n")

    for entry in manifest["cases"]:
        print(
            "HOSTILE-CASE: id={id} class={cls} expect={exp} scope={scope} "
            "entries={n} collapsed={col} digest={d}".format(
                id=entry["id"],
                cls=entry["class"],
                exp=entry["expect"],
                scope=entry["scope"],
                n=entry["entries"],
                col="yes" if entry["collapsed"] else "no",
                d=entry["corpus_digest"][:16],
            )
        )
    print(f"HOSTILE-GEN: cases={len(manifest['cases'])} manifest={path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
