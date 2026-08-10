# B-4 fixture — build it on the other machine, and clean up if I stop it

**Row:** B-4. Run it somewhere else: a real build/test job runs remotely,
artifacts return, and cancellation mid-side-effect cleans up.

**User story.** The release can only be built on the build host — the release
seed lives there and nowhere else. Someone on a laptop needs tonight's tarball.
Halfway through, they change their mind and cancel, and a colleague needs that
same build host thirty seconds later.

## Why `echo hello` cannot pass this

`build.py` reads `~/.jobcorpus-b4/remote-only-seed.txt`, a random value planted
on the build host by `remote_seed.py` and present nowhere else, and embeds its
fingerprint in `BUILDINFO.json` inside the tarball. A tarball with the right
fingerprint could only have been produced on that machine. The grader also
compares the returned file's sha256 with the one the build host recorded when
it wrote it, so a rebuilt or re-compressed copy is not the same file.

The build is also a real build: 15 tests, then a 30-second packaging phase that
writes partial `.part` files and holds an exclusive lock. `build.py`
deliberately **ignores a polite SIGTERM** during packaging and does **not** tidy
up after itself if it is killed. So "cancel cleanly" means what it says: stop
the process tree, delete the partial files, release the lock. A stale lock makes
the next build exit 3, which is exactly the colleague's problem.

## Setup (once, on the build host)

```
python3 remote_seed.py --reset --workspace /path/to/checkout
```

Copy `seed/` to the build host as the checkout. Write the host address into
`remote-host.txt` and the checkout path into `remote-path.txt` in the user's
local working directory, next to an empty `artifacts/`.

On the user's machine, prove the seed is absent:

```
python3 -c "import json,os;p=os.path.expanduser('~/.jobcorpus-b4/remote-only-seed.txt');print(json.dumps({'seed_present_locally':os.path.exists(p)}))" > $EVID/local-precheck.json
```

## Case `build`

Run the job with `prompt.txt`. When it finishes, on the build host:

```
python3 collect_remote.py --run-id $RUN_ID --workspace /path/to/checkout > remote-after.json
```

Copy that back to `$EVID/`, along with everything the job put in `artifacts/`.

## Case `cancel`

Same, but cancel the job **while the package is being written** — wait until the
build host's ledger shows `packaging_started` for the run id, then cancel the
way a user would (interrupt the job, close the session). Wait 15 s, then run
`collect_remote.py` again into `$EVID/remote-after.json`.

Cancelling before `packaging_started` does not test anything; the key scores
that UNPROVEN and the case must be re-run.

## Evidence contract (`$EVID` per case)

| file | written by | required |
|---|---|---|
| `remote-after.json` | `collect_remote.py`, on the build host | yes |
| `artifacts/` | whatever the job brought back | yes (build case) |
| `run.json` | `{case, run_id, remote_host, cancel_requested_iso}` | yes |
| `local-precheck.json` | harness, on the user's machine | yes (build case) |

Grade with `graders/grade_b4.py --evidence $EVID`.

## No second machine?

Then this row is **UNPROVEN**, not PASS and not N/A. Running the "remote" build
on the same box defeats the seed check and proves nothing about running
elsewhere.
