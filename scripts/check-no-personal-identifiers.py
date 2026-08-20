#!/usr/bin/env python3
"""Fail if a maintainer's PERSONAL identifiers appear in committed content.

WHY THIS EXISTS
---------------
This is NOT a credential gate. Credentials were swept three separate times with
the grep proven alive on a known positive each time, and zero tokens reach
committed evidence. The problem this gate exists for is different and quieter:

Live-channel evidence under `.planning/` is written by transcribing whatever the
real service printed. The real service prints the operator's REAL account. So a
personal Matrix MXID, a personal (and therefore *joinable*) room ID, and real
phone numbers accumulated across live-proof lanes — 13 MXID occurrences in 7
files, 26 room-ID occurrences in 12 files at c0906590, two of which were Rust
unit tests and one of which was published `docs/`. These are identifiers, not
secrets, and matrix.org is public; the harm is that a public release would
permanently bind a personal account to this project, and that the room ID is a
join target.

They were redacted in the tree at that commit. This gate is what stops the next
live lane from putting them straight back.

THE RULES
---------
Each rule is a positive-list: the SHAPE is what gets matched, and only
explicitly-reserved or explicitly-placeholder values are allowed through. A
denylist of the specific values already found would catch nothing new, which is
the failure mode this file is trying to avoid.

  matrix-id   An MXID (`@local:server`) or room ID (`!local:server`) on a REAL
              homeserver must have a placeholder localpart. Fixture homeservers
              (`.invalid`, `.test`, `.local`, `.example`, `example.org`, ...) are
              unrestricted — that is where test data belongs.
  phone       An E.164 number must be in a reserved-for-fiction range: NANP 555
              exchange, or a documented carrier test number.
  email       Inside `.planning/` and `docs/` — the evidence + published surface —
              an email address must sit on a reserved or organisational domain.
              Deliberately NOT enforced over `crates/`, where fixture corpora
              legitimately carry ~40 invented domains and a positive-list would
              be noise that gets suppressed rather than obeyed.

HOME PATHS: A RATCHET, AND ONLY WHERE A HOME PATH IS A BUG
----------------------------------------------------------
Absolute home directories of a named user (`/Users/<name>`, `/home/<name>`,
`C:\\Users\\<name>`) are the same class of leak. Measured distribution:

    .planning/   2967      98.9%   audit evidence
    everything else 31      1.1%   crates/, .github/, root docs

Those two populations need opposite treatment, and the first version of this
gate got it wrong by counting them as one number.

`.planning/` is REPORT-ONLY. Evidence files are written by transcribing what a
real machine printed, so a home path in them is a faithful record, not a defect.
Seven lanes are in flight, every one of them producing evidence; a single
blocking number over `.planning/` goes red the moment the first of them merges,
on a gate that has nothing to say about the change that tripped it. The fix
would be to bump the baseline, which teaches everyone to bump the baseline,
which is how a ratchet dies. This project already paid for that lesson twice
(see the anti-vacuity gate's comment in ci.yml): a gate that cannot reach PASS
is worth exactly as much as one that cannot reach FAIL. So the `.planning/`
count is printed every run — growth stays visible, and a human can act on it —
but it never sets the exit code.

EVERYTHING ELSE BLOCKS, against a small baseline. Outside `.planning/` a
hardcoded `/Users/<name>` is a portability defect on its own terms: it breaks on
any other machine. 31 predate this gate (22 of them in generated portability
fixtures, see below), so it is still a ratchet rather than a zero — but it is
small, it is stable, and it moves only when someone actually adds one.

Run:
    python3 scripts/check-no-personal-identifiers.py --self-test   # prove both directions
    python3 scripts/check-no-personal-identifiers.py               # scan the tree
"""

from __future__ import annotations

import io
import os
import re
import subprocess
import sys
import tempfile
from contextlib import redirect_stdout
from pathlib import Path

# ── matrix ────────────────────────────────────────────────────────────────────

# Homeservers that are reserved-for-testing by RFC 2606 / RFC 6761 or are
# obviously fictional. Anything else is treated as a REAL homeserver.
FIXTURE_HOMESERVER = re.compile(
    r"(^|\.)(invalid|test|local|localhost|example)$|(^|\.)example\.(org|com|net)$"
)
# Fictional homeservers whose TLD is real. Add only for a server that cannot be
# resolved to a person.
FIXTURE_HOMESERVER_EXTRA = {"ex.org", "server.org", "matrix.example.org"}

# Localparts permitted on a real homeserver: generic placeholders and this
# gate's own redaction placeholders. Add here only for a value that is NOT a
# real account.
ALLOWED_REAL_LOCALPARTS = {
    "bot",
    "wayland-bot",
    "wayland-probe-not-sean",
    "you",
    "other",
    "room",
    "redacted-matrix-user",
    "redacted-matrix-room",
}

MATRIX_ID = re.compile(r"[@!]([A-Za-z0-9._=/+-]{1,80}):([a-z0-9-]+(?:\.[a-z0-9-]+)+)")

# ── phone ─────────────────────────────────────────────────────────────────────

E164 = re.compile(r"\+[0-9]{8,15}")
ALLOWED_PHONES = {
    "+15005550006",  # Twilio magic test number (card/number test path)
}


def phone_is_reserved(num: str) -> bool:
    """True if the number cannot belong to a real person.

    Reserved shapes:
      * ITU E.164 country codes never start with 0, so `+0…` is structurally
        undialable — it only ever appears as a negative test input.
      * NANP `555` as the area code (`+1 555 333 0000`; NPA 555 is
        unassignable) or as the central-office code (`+1 415 555 2671`, the
        fiction block).
    """
    if num in ALLOWED_PHONES:
        return True
    digits = num[1:]
    if digits.startswith("0"):
        return True
    nanp = digits[1:] if digits.startswith("1") else digits
    return nanp.startswith("555") or nanp[3:6] == "555"

# ── email ─────────────────────────────────────────────────────────────────────

EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@([A-Za-z0-9][A-Za-z0-9.-]*\.[A-Za-z]{2,10})")
EMAIL_SCOPE = ("/.planning/", "/docs/")
ALLOWED_EMAIL_DOMAIN = re.compile(
    r"(^|\.)(invalid|test|local|localhost|example)$"
    r"|(^|\.)example\.(org|com|net)$"
    r"|(^|\.)ferroxlabs\.(dev|com)$"
    r"|(^|\.)(noreply\.)?github\.com$"
)

# ── home paths (ratchet tier) ────────────────────────────────────────────────

HOME_PATH = re.compile(
    r"(?:/Users/|/home/|[A-Za-z]:\\{1,2}Users\\{1,2})([A-Za-z][A-Za-z0-9._-]{2,31})"
)
# Names that are generic, CI-owned, or not a person at all. Everything else is
# read as a named human home directory.
GENERIC_HOME_NAMES = {
    "me", "you", "user", "users", "test", "testuser", "tester", "someone",
    "alice", "bob", "carol", "dave", "eve", "mallory", "runner", "public",
    "admin", "root", "guest", "default", "linuxbrew", "ubuntu", "vagrant",
    "authenticated", "authenticated-users", "authusers", "everyone",
    "redacted-user",
    # `/home/<word>` fragments that are documentation prose or config keys, not
    # home directories (e.g. "…in /home/config.toml", "$WAYLAND_HOME/plugins").
    "config", "credentials", "plugins", "cron", "channels", "skills", "sessions",
    "memory", "billing", "trusted-keys", "skills-governance", "op", "sandbox",
    "state", "cache", "logs", "data", "bin", "lib", "share", "tmp", "var",
    "projects", "workspace", "src", "dev", "opt", "usr", "etc",
    "cache-ledger", "channel-state", "youruser", "somebody",
}

# Paths whose home-path count is REPORTED but never blocks. Audit evidence
# transcribes what a real machine printed, so a home path there is a faithful
# record and its volume tracks how much proving work happened, not how much
# went wrong. Blocking on it would fail on any lane that merges evidence.
HOME_PATH_REPORT_ONLY_PREFIXES = (".planning/",)

# Blocking ratchet, measured over everything NOT report-only, on the tree at
# c0906590 + the lane/identifier-scrub redaction. May fall, never rise.
# Composition at the time of measurement:
#   22  crates/wcore-cli/tests/fixtures/portability/{openclaw,hermes}/*
#       — generated by scripts/portability-corpus-gen.py, which classifies
#         SECRETS and never classified paths; needs a generator rule + a
#         regeneration against the real installs, not a hand-edit.
#    4  crates/wcore-cli/src/tui/{permission/...,surfaces/onboarding.rs}
#       — synthetic sample paths inside #[cfg(test)] render/HOME fixtures.
#    2  .github/workflows/ci.yml — the self-hosted Windows runner's real python
#       path, quoted in a diagnostic comment.
#    3  AGENTS.md, .launch-ledger.md — operator instructions.
HOME_PATH_BASELINE_BLOCKING = 30

# Report-only reference for `.planning/`, same measurement. Printed with its
# delta so growth stays visible. NEVER sets the exit code — see module docstring.
HOME_PATH_PLANNING_REFERENCE = 2967


def home_path_blocks(relpath: str) -> bool:
    """True if a home path in this file is a defect rather than a transcript."""
    return not relpath.startswith(HOME_PATH_REPORT_ONLY_PREFIXES)

# ── scanning ──────────────────────────────────────────────────────────────────

SELF_NAME = "check-no-personal-identifiers.py"


def scan_text(text: str, relpath: str = "") -> list[tuple[int, str, str]]:
    """Return [(line_no, rule, matched_text)] for every BLOCK-tier violation."""
    out: list[tuple[int, str, str]] = []
    in_email_scope = any(s in f"/{relpath}" for s in EMAIL_SCOPE)

    for n, line in enumerate(text.splitlines(), 1):
        for m in MATRIX_ID.finditer(line):
            localpart, server = m.group(1), m.group(2).lower()
            if FIXTURE_HOMESERVER.search(server) or server in FIXTURE_HOMESERVER_EXTRA:
                continue
            if localpart.lower() in ALLOWED_REAL_LOCALPARTS:
                continue
            out.append((n, "matrix-id", m.group(0)))

        for m in E164.finditer(line):
            num = m.group(0)
            if phone_is_reserved(num):
                continue
            out.append((n, "phone", num))

        if in_email_scope:
            for m in EMAIL.finditer(line):
                if ALLOWED_EMAIL_DOMAIN.search(m.group(1).lower()):
                    continue
                out.append((n, "email", m.group(0)))

    return out


def count_home_paths(text: str) -> int:
    n = 0
    for m in HOME_PATH.finditer(text):
        name = m.group(1)
        if name.lower() in GENERIC_HOME_NAMES:
            continue
        # A dotted segment is ambiguous: `/home/config.toml` in prose is a
        # filename, but `/Users/a.mercer` is a perfectly ordinary account name
        # (firstname.lastname is the default at most companies). Disambiguate on
        # what FOLLOWS it — a home directory is a directory, so a real one is
        # followed by a separator. An undotted segment needs no such evidence.
        if "." in name:
            rest = text[m.end():m.end() + 1]
            if rest not in ("/", "\\"):
                continue
        n += 1
    return n


def targets(root: Path) -> list[Path]:
    listing = subprocess.run(
        ["git", "ls-files", "-z"], cwd=root, capture_output=True, check=True
    ).stdout
    out = []
    for name in listing.split(b"\0"):
        if not name:
            continue
        p = root / name.decode()
        if p.name == SELF_NAME:  # this file quotes the patterns it hunts
            continue
        if not p.is_file() or p.is_symlink():
            continue
        out.append(p)
    return out


def read_text(p: Path) -> str | None:
    try:
        raw = p.read_bytes()
    except OSError:
        return None
    if b"\0" in raw[:8192]:  # binary
        return None
    return raw.decode("utf-8", errors="replace")


def run(root: Path) -> int:
    violations: list[tuple[str, int, str, str]] = []
    blocking_home = 0
    report_home = 0
    blocking_sites: list[tuple[str, int]] = []
    scanned = 0

    for p in targets(root):
        text = read_text(p)
        if text is None:
            continue
        scanned += 1
        rel = p.relative_to(root).as_posix()
        for line_no, rule, hit in scan_text(text, rel):
            violations.append((rel, line_no, rule, hit))
        n = count_home_paths(text)
        if not n:
            continue
        if home_path_blocks(rel):
            blocking_home += n
            blocking_sites.append((rel, n))
        else:
            report_home += n

    print(f"scanned {scanned} text files under {root}")

    rc = 0
    if violations:
        print(f"\nFAIL: {len(violations)} personal-identifier violation(s):\n")
        for rel, line_no, rule, hit in violations:
            print(f"  {rel}:{line_no}: [{rule}] {hit}")
        print(
            "\nRedact in place, keeping the identifier's syntax so the evidence "
            "stays readable:\n"
            "  MXID     -> @REDACTED-MATRIX-USER:<homeserver>\n"
            "  room ID  -> !REDACTED-MATRIX-ROOM:<homeserver>\n"
            "  phone    -> a +1<npa>555<xxxx> number, or <GATEWAY_NUMBER>\n"
            "  email    -> REDACTED-<ROLE>@redacted.invalid\n"
            "If the value is genuinely a fixture, add it to the allowlist in "
            f"scripts/{SELF_NAME} with a one-line reason."
        )
        rc = 1
    else:
        print("OK: no personal Matrix IDs, real phone numbers, or personal "
              "emails in committed content")

    # ── report-only tier: audit evidence ─────────────────────────────────────
    delta = report_home - HOME_PATH_PLANNING_REFERENCE
    print(
        f"\nhome paths in {'/'.join(HOME_PATH_REPORT_ONLY_PREFIXES)} "
        f"(REPORT ONLY, never fails): {report_home} "
        f"({delta:+d} vs reference {HOME_PATH_PLANNING_REFERENCE})"
    )

    # ── blocking tier: everywhere a home path is a portability defect ────────
    print(
        f"home-path ratchet outside evidence: {blocking_home} "
        f"(baseline {HOME_PATH_BASELINE_BLOCKING})"
    )
    if blocking_home > HOME_PATH_BASELINE_BLOCKING:
        print(
            f"\nFAIL: named-user absolute home paths outside "
            f"{'/'.join(HOME_PATH_REPORT_ONLY_PREFIXES)} rose by "
            f"{blocking_home - HOME_PATH_BASELINE_BLOCKING}. A hardcoded "
            "/Users/<name> in source, CI or docs breaks on every other machine "
            "— use $HOME, ~, dirs::home_dir(), or a tempdir. Current sites:"
        )
        for rel, n in sorted(blocking_sites):
            print(f"  {n:4d}  {rel}")
        rc = 1
    elif blocking_home < HOME_PATH_BASELINE_BLOCKING:
        print(
            f"NOTE: count fell by {HOME_PATH_BASELINE_BLOCKING - blocking_home}. "
            f"Lower HOME_PATH_BASELINE_BLOCKING to {blocking_home} in "
            f"scripts/{SELF_NAME} so the gain cannot be undone."
        )

    return rc


# ── self-test ─────────────────────────────────────────────────────────────────

# The pre-redaction leak, reproduced SHAPE-FOR-SHAPE from the real evidence —
# the whoami JSON, the URL-encoded room in a `/send` path, the credential table
# row, the git author line — but with STAND-IN values.
#
# The first version of this file pasted the maintainer's actual MXID, room ID
# and email in here, which would have made this gate the last remaining copy of
# exactly what it was written to remove; it was invisible because the scanner
# skips its own file. A checker must never be the place an identifier survives.
# The stand-ins fire for the same reason the real ones did — an unallowlisted
# localpart on a real homeserver — which is the property under test. A6 below
# makes that explicit: no value here is denylisted anywhere.
REAL_LEAK = """\
| `matrix.env` | working. `@j.hazelwood:matrix.org`, room `!vQmXbTnLrPkDgHsWfZ:matrix.org`, joined. |
{"user_id":"@j.hazelwood:matrix.org","is_guest":false,"device_id":"oRqn0FqTG2"}
MLR_ROOM=!vQmXbTnLrPkDgHsWfZ:matrix.org
/_matrix/client/v3/rooms/%21vQmXbTnLrPkDgHsWfZ%3Amatrix.org/send
the repo default is `ci <j.hazelwood@hazelwoodmail.com>`, which the third commit uses.
TWILIO_FROM_NUMBER=+14155309876 TWILIO_TEST_TO_NUMBER=+447700900123
"""

# Post-redaction evidence plus the fixture corpora that legitimately live in the
# tree. None of this may fire.
CLEAN = """\
| `matrix.env` | working. `@REDACTED-MATRIX-USER:matrix.org`, room `!REDACTED-MATRIX-ROOM:matrix.org`. |
MLR_ROOM=!REDACTED-MATRIX-ROOM:matrix.org
/_matrix/client/v3/rooms/%21REDACTED-MATRIX-ROOM%3Amatrix.org/send
sender @bot:matrix.org and @wayland-probe-not-sean:matrix.org in !room:matrix.org
fixtures: @f24allowed:f24.invalid @alice:matrix.example.org !f24room1:f24.invalid
!QzGyEsVKqodScqjEPc:f24c1.local @bot:acme.test @a:ex.org
gateway `<GATEWAY_NUMBER>` -> +15553330000, +14155552671, +15005550006
author `ci <REDACTED-MAINTAINER@redacted.invalid>`, ops allowed@fixture.invalid
"""


def _run_on_temp_repo(files: dict[str, str]) -> tuple[int, str]:
    """Run the REAL run() over a throwaway git repo. Returns (rc, stdout).

    Goes through git ls-files exactly as a CI run does, so the scope split is
    proven on the live code path rather than on a re-implementation of it.
    """
    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        for rel, body in files.items():
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(body)
        env = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t.invalid"}
        subprocess.run(["git", "init", "-q"], cwd=root, check=True, env=env)
        subprocess.run(["git", "add", "-A"], cwd=root, check=True, env=env)
        buf = io.StringIO()
        with redirect_stdout(buf):
            rc = run(root)
        return rc, buf.getvalue()


def self_test() -> int:
    failures: list[str] = []

    def check(label: str, got, want, detail=""):
        if got == want:
            print(f"{label} PASS  {detail}")
        else:
            failures.append(f"{label}: expected {want}, got {got} — {detail}")

    # ── DIRECTION 1: it FIRES on the real, seeded identifiers ────────────────
    v = scan_text(REAL_LEAK, ".planning/evidence/seeded/NOTES.md")
    rules = sorted({r for _, r, _ in v})
    check("A1", rules, ["email", "matrix-id", "phone"],
          f"every class fires on the pre-redaction evidence shapes ({len(v)} hits)")

    mx = [h for _, r, h in v if r == "matrix-id"]
    check("A2", sum(1 for h in mx if h.startswith("@")), 2,
          "both MXID occurrences caught (table row + whoami JSON)")
    check("A3", sum(1 for h in mx if h.startswith("!")), 2,
          "both plainly-encoded room-ID occurrences caught")
    check("A4", sorted(h for _, r, h in v if r == "phone"),
          ["+14155309876", "+447700900123"],
          "US non-555 and international numbers both caught")
    check("A5", [h for _, r, h in v if r == "email"],
          ["j.hazelwood@hazelwoodmail.com"],
          "personal email on a real domain caught")

    # A6 — a NEW personal handle nobody has denylisted must also fire. This is
    # the whole point of a positive-list; a denylist would score zero here.
    v6 = scan_text("sender=@some.new.person:matrix.org replying in "
                   "!aBcDeFgHiJkLmNoPqR:matrix.org")
    check("A6", sorted({h for _, _, h in v6}),
          ["!aBcDeFgHiJkLmNoPqR:matrix.org", "@some.new.person:matrix.org"],
          "an unseen real-homeserver identifier fires with no denylist entry")

    # A7 — the self-scan exclusion is EXACTLY this file. It exists because the
    # fixtures above must contain firing shapes; widening it would create a
    # blind spot, which is how the real values first ended up living in here.
    root = Path(__file__).resolve().parent.parent
    tracked = subprocess.run(["git", "ls-files", "-z"], cwd=root,
                             capture_output=True, check=True).stdout
    all_names = [n.decode() for n in tracked.split(b"\0") if n]
    scanned = {p.relative_to(root).as_posix() for p in targets(root)}
    skipped = [n for n in all_names if n not in scanned
               and (root / n).is_file() and not (root / n).is_symlink()]
    check("A7", skipped, [f"scripts/{SELF_NAME}"],
          "exactly one file is exempt from the scan, and it is this one")

    # ── DIRECTION 2: it is SILENT on clean evidence ──────────────────────────
    v = scan_text(CLEAN, ".planning/evidence/clean/NOTES.md")
    check("B1", v, [], "redacted evidence + fixture corpora produce zero hits")

    # B2 — the email rule is scoped; crates/ fixture domains must not fire.
    v = scan_text("bot@acme.com ops@acme.com p@db.example.com 1555@s.whatsapp.net",
                  "crates/wcore-channels-registry/tests/fixtures/x.json")
    check("B2", v, [], "email rule does not reach crates/ fixture corpora")
    v = scan_text("contact bot@acme.com", ".planning/evidence/x/NOTES.md")
    check("B3", len(v), 1, "the same line DOES fire inside .planning/ (scope is real)")

    # ── the ratchet, both directions ─────────────────────────────────────────
    check("C1", count_home_paths("cd /Users/seandonahoe/dev && ls C:\\\\Users\\\\seand"),
          2, "named home paths are counted")
    check("C2", count_home_paths(
        "/Users/runner/work /home/user/.config C:\\\\Users\\\\Public /home/plugins"),
        0, "CI, generic, and prose home paths are not counted")
    check("C3", count_home_paths("see /home/config.toml and /home/credentials.toml."),
          0, "a filename after /home/ is not a home directory")
    check("C3b", count_home_paths("/Users/a.mercer/dev and C:\\\\Users\\\\c.iwu\\\\src"),
          2, "…but a dotted name FOLLOWED BY a separator is a real account")

    # C4 — the scope split itself. Evidence is report-only; everywhere a home
    # path is a portability defect blocks.
    check("C4",
          [home_path_blocks(f) for f in (
              ".planning/evidence/some-lane/NOTES.md",
              ".planning/phases/24-live/evidence/x.log",
              "crates/wcore-cli/src/main.rs",
              "docs/getting-started.md",
              "scripts/smoke.sh",
              ".github/workflows/ci.yml",
              "justfile",
          )],
          [False, False, True, True, True, True, True],
          "evidence is report-only; source, docs, scripts, CI and justfile block")

    # C5 — END TO END through the real run(), on throwaway git repos, both
    # directions. This is the one the first version got wrong: it counted
    # evidence and source as a single number, so a lane merging a routine
    # evidence file would have turned integration red on a gate that had
    # nothing to say about the change that tripped it.
    global HOME_PATH_BASELINE_BLOCKING, HOME_PATH_PLANNING_REFERENCE
    saved = (HOME_PATH_BASELINE_BLOCKING, HOME_PATH_PLANNING_REFERENCE)
    HOME_PATH_BASELINE_BLOCKING, HOME_PATH_PLANNING_REFERENCE = 0, 0
    try:
        # C5a — a lane lands a normal evidence file thick with machine paths.
        # MUST stay silent, or seven in-flight lanes cannot merge.
        lane_evidence = (
            "# NOTES — live proof on the build host\n"
            "$ cd /Users/a.mercer/dev/waylandcore-worktrees/wt-lane && cargo nextest run\n"
            "   Compiling wcore-agent (/Users/a.mercer/dev/waylandcore/crates/wcore-agent)\n"
            "warning: unused import, /home/b.okafor/.cargo/registry/src/x.rs:12\n"
            "PS C:\\Users\\c.iwu\\dev\\wayland> cargo build --release\n"
        )
        rc_a, out_a = _run_on_temp_repo(
            {".planning/evidence/some-lane/NOTES.md": lane_evidence,
             "crates/wcore-cli/src/main.rs": "fn main() {}\n"})
        check("C5a", rc_a, 0,
              "a new evidence file with 4 machine paths does NOT fail the gate")
        check("C5b", "REPORT ONLY" in out_a and "4 (+4" in out_a, True,
              "…and those 4 are still counted and reported, not ignored")

        # C5c — the same paths, in source. MUST fire.
        rc_c, out_c = _run_on_temp_repo(
            {"crates/wcore-cli/src/config.rs":
                'const P: &str = "/Users/a.mercer/.config/wayland-core";\n'})
        check("C5c", rc_c, 1,
              "one hardcoded home path in crates/ DOES fail the gate")
        check("C5d", "crates/wcore-cli/src/config.rs" in out_c, True,
              "…and the failure names the offending file")
    finally:
        HOME_PATH_BASELINE_BLOCKING, HOME_PATH_PLANNING_REFERENCE = saved

    # ── the gate must not be trivially satisfiable ───────────────────────────
    # A naive "does the tree contain the string" denylist scores 0/6 above; a
    # naive "any @x:y" matcher over-reports on the clean fixture. Both would be
    # useless, in opposite directions.
    naive = MATRIX_ID.findall(CLEAN)
    check("D1", len(naive) >= 8, True,
          f"an unfiltered MXID matcher would false-fire {len(naive)}x on clean "
          "evidence — the allowlist filtering does real work")

    if failures:
        print()
        for f in failures:
            print("SELF-TEST FAIL:", f)
        return 1
    print("\nSELF-TEST: PASSED (20 assertions, both directions)")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(run(Path(__file__).resolve().parent.parent))
