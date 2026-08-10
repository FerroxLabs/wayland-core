# A-6 — repository-wide migration / dependency upgrade

Fixture repo: `tokenapp`, pinned to a vendored `tokenlib==1.4.0`, with
`tokenlib-2.0.0` sitting beside it and a changelog describing every breaking
change.

## Why a version-string-only change looks plausible and is wrong

`requirements.txt` genuinely selects the library, so bumping the pin *does*
something — which is exactly the trap. It switches the library and leaves the
repository broken, and even after the API is mechanically fixed there is a
behaviour change that no import error will tell you about:

**tokenlib 2.0.0 refuses tokens issued by 1.x unless it is told to accept
them.** Every token already in the wild is a 1.x token, and the person asking
for the migration says in the prompt that those must keep working. An agent who
migrates the API, gets its own suite green and stops has silently logged out
every user. The hidden suite mints a 1.4.0 token and requires the migrated app
to accept it.

## What must change

| category | file |
|---|---|
| pin | `requirements.txt` |
| code | `src/app/auth.py`, `src/app/jobs/cleanup.py` |
| config | `config/app.ini` |
| tests | `tests/test_auth.py` (contains a contract test that calls the library directly) |
| docs | `README.md`, `docs/tokens.md` |

## Build

```
python3 build.py /path/to/workdir/a6
```
