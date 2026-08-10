# B-2 fixture — the month-end report, and the provider dies halfway through

**Row:** B-2. Survive a provider failure mid-task. Graded on **continuity of
the work**, never on failover mechanics.

**User story.** Finance is waiting on the reconciliation report. The person who
asked for it said, once, in one sentence, which account code to stamp on every
row and how to round. Halfway through the job the model provider falls over.
What must survive is not the connection — it is the work: the half already done
still standing, the half still to come consistent with it and with what the
user said at the start. A run that comes back from the outage having forgotten
the account code has not survived anything.

## How the failure is induced

`provider_proxy.py` sits between the program under test and the real provider.
It relays faithfully until `--trigger-path` exists in the project, then breaks
the connection for real: TCP reset, HTTP 503, or a hang. Pointing the trigger
at `ledger/schema.json` means the outage always lands in the same place — the
moment the first half of the job is on disk and the second half has not started.

The proxy never records request or response bodies and drops credential
headers. It observes traffic; it does not read it.

## Cases

| case | provider |
|---|---|
| `control` | proxy in place, no fault injected |
| `fault-reset` | connection reset from the trigger onward, for 6 requests, then healed |
| `fault-503` | `--fault http503` |
| `fault-timeout` | `--fault timeout --hang-seconds 90` |

`control` is the positive control: it proves the job is achievable and the
report is gradeable. If `control` is not PASS the fault cases are UNPROVEN.

Run at least `control` plus `fault-reset`. The other two are the same row with
a different failure shape and are recorded separately.

## Procedure

1. `python3 seed_workspace.py --dest $WS`
2. Start the proxy:
   ```
   python3 provider_proxy.py --port $PORT --upstream $REAL_PROVIDER_BASE_URL \
       --ledger $EVID/proxy-ledger.jsonl \
       --trigger-path $WS/ledger/schema.json --fault reset --fault-requests 6
   ```
3. Configure the program under test to reach the provider through
   `http://127.0.0.1:$PORT`. Run it with `env -u API_KEY -u FLUX_API_KEY` and
   give it `prompt.txt` **once**.
4. The harness watches `$WS/ledger/schema.json`; the moment it appears, copy the
   whole project to `$EVID/workspace-at-fault/`. That snapshot is what makes the
   continuity check possible.
5. Let the run finish. Do not re-brief it. If a human has to restate the task,
   record `"user_restated": true` in `run.json` — that is a FAIL, not a rescue.

## Evidence contract (`$EVID`)

| file | written by | required |
|---|---|---|
| `workspace-final/` | harness copy of `$WS` at the end | yes |
| `workspace-at-fault/` | harness copy of `$WS` when the outage began | yes (fault cases) |
| `proxy-ledger.jsonl` | the proxy | yes (fault cases) |
| `git-log.txt` | `git -C $WS log --format=%H %s --name-only` | yes |
| `run.json` | `{case, exit_code, wall_seconds, user_restated}` | yes |

Grade with `graders/grade_b2.py --evidence $EVID`. The grader runs the hidden
acceptance test in `keys/b-2/acceptance/`, which executes the delivered code —
grade on a throwaway machine or in a container.
