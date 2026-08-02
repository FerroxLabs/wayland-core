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

HOME PATHS ARE A RATCHET, NOT A BLOCK
-------------------------------------
Absolute home directories of a named user (`/Users/<name>`, `/home/<name>`,
`C:\\Users\\<name>`) are the same class of leak, but there are 2998 of them
across 450+ files at c0906590 — transcribed shell prompts and cargo paths in
logs that predate this gate. Blocking would make this gate permanently red, and
a gate that cannot reach PASS is worth exactly as much as one that cannot reach
FAIL. So they are counted against a recorded baseline: the count may fall, never
rise. Cleaning the backlog is a separate decision (it is a 450-file rewrite of
audit evidence, and every open lane branch would conflict); this at least stops
the pile growing.

Run:
    python3 scripts/check-no-personal-identifiers.py --self-test   # prove both directions
    python3 scripts/check-no-personal-identifiers.py               # scan the tree
"""

from __future__ import annotations

import re
import subprocess
import sys
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

# Measured on the tree at c0906590 + the lane/identifier-scrub redaction.
# May fall, never rise.
HOME_PATH_BASELINE = 2998

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
        # `/home/config.toml` in prose is a filename, not a home directory.
        if "." in name or name.lower() in GENERIC_HOME_NAMES:
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
    home_paths = 0
    scanned = 0

    for p in targets(root):
        text = read_text(p)
        if text is None:
            continue
        scanned += 1
        rel = p.relative_to(root).as_posix()
        for line_no, rule, hit in scan_text(text, rel):
            violations.append((rel, line_no, rule, hit))
        home_paths += count_home_paths(text)

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

    print(f"\nhome-path ratchet: {home_paths} (baseline {HOME_PATH_BASELINE})")
    if home_paths > HOME_PATH_BASELINE:
        print(
            f"FAIL: named-user absolute home paths rose by "
            f"{home_paths - HOME_PATH_BASELINE}. Do not add new ones: use $HOME, "
            "~, or a tempdir. If a cleanup lowered the count, lower "
            f"HOME_PATH_BASELINE in scripts/{SELF_NAME} to lock the gain in."
        )
        rc = 1
    elif home_paths < HOME_PATH_BASELINE:
        print(
            f"NOTE: count fell by {HOME_PATH_BASELINE - home_paths}. Lower "
            "HOME_PATH_BASELINE to this value so the gain cannot be undone."
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
    print("\nSELF-TEST: PASSED (14 assertions, both directions)")
    return 0


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(run(Path(__file__).resolve().parent.parent))
