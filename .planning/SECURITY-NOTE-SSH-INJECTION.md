# Security note — remote command injection in the ssh exec backend

**Found:** 2026-07-28 by `lane/25-hosts`, while closing Phase 25 Criteria 2 and 4.
**Status: FIXED** (merged `6861b3aa`). **Never shipped to a customer** — see the exposure
determination below, which is the fact that decides whether this needs an advisory.

## The defect

```
backend scan --task-id 'x;id>/tmp/w;echo y'
```
executed `id` **as root on the far end**. `ssh` carries no argv — it concatenates its arguments
and the far end's **login shell re-parses the result** — so argv-mode safety on the near side
buys nothing once the string crosses the connection. The module's own documentation asserted
that argv safety was "extended across the connection". It was not.

Proved through the **shipped binary** with a negative control, not by reading the source.

## Why the guard did not catch it, which is the part worth internalising

`the_module_contains_no_shell_string_execution_path` **passed the entire time** — because it
**greps its own source**. It asserted that the module contains no shell-string execution
*syntax*, which was true, while the module's actual behaviour was to hand a shell string to a
remote shell. A test that inspects source text cannot observe what a remote peer does with the
bytes it receives.

This is a **tautological gate**, one of the named self-passing classes in `LANE-BRIEF` §3.2, and
it is the most expensive instance this program has found: it made a genuine remote code
execution vector look actively covered for as long as the file existed.

**The rule this justifies:** a security property that crosses a process, host or trust boundary
must be proved by **driving the boundary**, never by inspecting the source on one side of it.

## The fix

`posix_quote` on the far-end command construction, plus a **round-trip test carrying its own
positive control** — it constructs a hostile task id, sends it across a real connection, and
asserts both that the injected command did NOT run and that the benign payload DID. The positive
control is what stops the test degrading into a second tautology.

Two related defects fixed in the same lane:
- An **empty task input vanished from the wire**, shifting argv left — a silent semantic change.
- The orphan sweep reported **`0 (MEASURED)` while an orphan was running** on Windows: msys `ps`
  rejects `-eo`, stderr was discarded, and the failure was swallowed by `|| true`. A structural
  false zero of the same shape as the cloud backend's metadata defect found earlier the same day.
  Now falls back to `ps -ef`, or reports `NOT MEASURED`.

## Exposure determination — measured, with the instrument checked first

`crates/wcore-exec-backend/src/backends/ssh.rs` first appears at `d0fc5095`.

```
git tag --contains d0fc5095   →  (empty)
```

**Instrument sanity, because an empty result from a broken query proves nothing:** the
repository has **36 tags**, and a positive control — the root commit — is contained in **all
36**. So the empty result is a genuine negative, not a query that cannot return anything.

**Conclusion: the ssh exec backend has never been part of a tagged release, including
`v0.12.25`. The injection was never exposed to a customer.** It is a pre-release defect found by
pre-release testing, which is the system working. No advisory, no CVE, no customer notification
is required.

**It would have shipped** in the release candidate had this lane not run it against a second
real host. It was not findable from the near side: the near-side code is argv-clean, and the
only way to see the defect is to observe what the far end's shell does with the bytes.
