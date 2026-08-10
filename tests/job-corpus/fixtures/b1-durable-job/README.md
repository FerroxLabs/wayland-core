# B-1 fixture — the warehouse handoff that must survive being killed

**Row:** B-1 (blocker). Complete a durable job across interruption: kill it
mid-task, disconnect, resume.

**User story.** Every night the warehouse hands off the day's ready orders to
the shipping service. Each handoff costs money: the service books a shipment
and bills for it, and it has no de-duplication and no cancel. The person who
starts this job goes home. The laptop lid closes, the SSH session drops, the
box reboots. When the job is picked up again the customer must not be charged
twice, and no order may be quietly left unshipped.

## Why the effects here are real

Correctness is decided by `shipping_service.py`, a separate process that is not
the program under test. It appends one line per HTTP request to a ledger that
lives **outside** the workspace the job is given, fsynced per line. Bookings
(`POST /register`) are effects; lookups (`GET /shipments`) are free and are
logged separately so a careful job can check itself without being punished for
it.

Nothing the program under test says about its own progress is an input to the
grade. The ledger is the world.

## Cases the harness must run

`N = 5` ready orders, so there are five booking boundaries. Run **eleven**
cases, each in a fresh workspace and with a fresh ledger:

| case id | interruption |
|---|---|
| `control` | none — the job runs to completion undisturbed |
| `k1-after` … `k5-after` | killed just **after** the K-th booking is acknowledged |
| `k1-before` … `k5-before` | killed **before** the K-th booking's reply is written — the shipment exists, the job never heard about it |

`control` exists to prove the job is achievable at all. If `control` does not
reach PASS the other ten cases are UNPROVEN, not FAIL — the fixture is broken,
not the product.

## Procedure per case

1. Pick a free port. Start the service:

   ```
   python3 shipping_service.py --port $PORT \
       --ledger $EVID/sink-ledger.jsonl \
       [--kill-at K --kill-variant before|after --pid-file $EVID/job.pid]
   ```

2. Seed a workspace:

   ```
   python3 seed_workspace.py --dest $WS --endpoint http://127.0.0.1:$PORT
   ```

3. Start the job under test in its own process group / job object, give it
   `prompt.txt` as the user's request, and write its pid to `$EVID/job.pid`
   **before** it can make its first request. Run it with
   `env -u API_KEY -u FLUX_API_KEY`.

4. The service kills the whole process tree at the configured boundary. After
   the kill, wait 5 s and confirm no process from that tree is alive
   (`$EVID/procs-after-kill.txt`). This is the disconnect: the terminal the job
   was started from is closed and never reused.

5. Resume from a **new** terminal with the resume invocation recorded in
   `$EVID/run.json` as `resume_cmd`. The resume invocation must not contain the
   task text, the order ids, or the endpoint. Re-supplying the instructions is
   not a resume, and the key scores it as a FAIL.

6. Collect evidence (below), then stop the service.

## Evidence contract (`$EVID` per case)

| file | written by | required |
|---|---|---|
| `sink-ledger.jsonl` | the shipping service | yes |
| `workspace-final/` | harness copy of `$WS` after the run | yes |
| `git-log.txt` | `git -C $WS log --format=%H %s --name-only` | yes |
| `git-status.txt` | `git -C $WS status --porcelain` | yes |
| `run.json` | harness: `{case, kill_at, kill_variant, resumed, resume_cmd, wall_seconds, exit_codes}` | yes |
| `procs-after-kill.txt` | harness: process table right after the kill | no (NOTE only) |

Grade with `graders/grade_b1.py --evidence $EVID`.
