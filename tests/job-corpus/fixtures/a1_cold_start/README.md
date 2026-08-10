# A-1 — cold install to a useful repository action

Fixture repo: `unitkit`, a two-function unit-conversion package with a working
test suite.

## Cold-start conditions the harness must impose

This row is not "can it edit a file". It is "a person who has never run this
before, on a machine that has never run it, ends up with a working change".

* A config/home directory that **does not exist** before the run.
* **No credentials of any kind pre-installed** — authentication happens during
  the run.
* **No system credential store reachable.** On Linux that means no
  `DBUS_SESSION_BUS_ADDRESS` and no secret-service; on macOS a login keychain
  that is not unlocked; on Windows no pre-seeded credential manager entry.
  This is what a fresh machine actually looks like, and it is the path that was
  a confirmed release blocker.
* No warm caches from an earlier run in the same config root.

## Build

```
python3 build.py /path/to/workdir/a1
```

Produces a git repo at that path on branch `main`, tagged `baseline`.
