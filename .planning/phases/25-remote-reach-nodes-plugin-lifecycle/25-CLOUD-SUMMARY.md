# Phase 25 — cloud lane summary

Closes Criterion 1's fourth surface and the cloud half of Criterion 4, using the Fly.io
credential Sean minted after lane/25 handed back.

**Branch:** `lane/25-cloud` · **SHA proved:** `5e620ef06487dc9477a2b10f2fe0045dd642c29d`
**Proof host:** `hetzner-dsm` (Ubuntu-2404-noble-amd64-base) · **Date:** 2026-07-28
**Ledger:** `evidence/25-cloud-ledger.txt` · **Raw transcripts:** `evidence/25-cloud-*.txt`

---

## The headline, stated plainly

The cloud backend was **broken, not merely unexercised**. 25-01 graded the leg
`UNEXERCISED` for want of a credential and described the code as "implemented and one token
away from running". It was not one token away. Three defects each independently prevented
the leg from working, and two of them would have produced a **false green** rather than a
failure once a token arrived. That is the most valuable thing this lane found, and it was
only findable by running the code.

| # | Severity | Defect | Would it have failed loudly? |
|---|---|---|---|
| 1 | HIGH | Machine create sent **no request body**; the vendor requires a `config` naming an image | Yes — HTTP 400 |
| 2 | HIGH | **No metadata was ever set**, yet the orphan scan filters on `metadata.wayland_task_nonce` | **No — silent false zero** |
| 3 | HIGH | The task **never ran on the machine**; stdout was the input echoed back by the controller | **No — a false EQUIVALENT** |

All three are fixed and each is proven closed by a control that could have failed.

---

## Criterion 1 — "the same task runs locally, in a container, over SSH, and on one hibernating cloud machine"

### Verdict: **MET**, with one qualification stated in full

The cloud surface now runs. Through the shipped binary at the SHA above:

- `backend list` reports cloud `yes / vendor_api_call` — "vendor API answered 200 for app
  wayland-f25-test machine listing". At 25-01 this row read `NO / credential_absent`.
- `backend run --backend cloud` exits **0**, machine `8ed9d7dc3e5618`, terminal `Success`,
  wall 9745 ms; the receipt passes integrity verification.
- `backend diff` over local + container + cloud at this commit: **EQUIVALENT**, no differing
  normalized fields.

**The qualification.** The ssh leg was **not re-run at this commit**. `WAYLAND_EXEC_SSH_TARGET`
is unset on `hetzner-dsm` and the `f25-ssh-target` host entry 25-01 used no longer exists. SSH
passed at 25-01's commit against a containerised sshd. So the four-surface claim is a
**composition across two commits**: local, container and cloud proven together here; ssh proven
earlier, elsewhere in the tree's history. I am recording that rather than implying one run
covered all four.

### What proves the hibernation was a real suspend and not a stop/start

This is the crux, because `suspend` versus `stop` is the *only* thing that makes the cloud
surface different from the three that already pass. Binding condition C1 forbids reporting a
stop as hibernation.

**State was carried across the transition and observed to survive.** Before suspending, the
backend writes a witness string into `/dev/shm` — a tmpfs, so it lives in guest RAM and nowhere
else — and reads the guest kernel's `boot_id` and uptime. After the resume it reads all three
back. From the receipt, verbatim:

```
suspended:suspended
resumed:started previous_state=suspended
ram-witness before=f25-ram-witness-f25-reference-nonce
            boot_id=d9b32269-d2e2-46cd-a840-6327465d7b04 uptime=1s
          / after=f25-ram-witness-f25-reference-nonce
            boot_id=d9b32269-d2e2-46cd-a840-6327465d7b04 uptime=3s
```

The witness survived and the boot id is byte-identical. The machine did not come back — it
resumed from RAM.

**The control that could have failed, and did, exactly where it should.** Both transitions were
driven on **one machine** (`80e9e51a1d42d8`) minutes apart, so the transition is the only
difference between the halves:

| signal | suspend/start | stop/start (control) |
|---|---|---|
| `/dev/shm` witness | `f25-witness-suspend` — **kept** | **`MISSING`** |
| guest `boot_id` | `080bbc1b…` → `080bbc1b…` | `080bbc1b…` → **`751ec4fb…`** |
| guest uptime | 1s → 4s | 4s → 4s |
| `start` `previous_state` | `"suspended"` | **`"stopped"`** |

**A weakness I am not smoothing over:** in the stop half, uptime read `4s` on *both* sides by
coincidence. The uptime clause **alone would not have caught that stop.** The witness, the boot
id and `previous_state` each did. That is the concrete, measured reason the discriminator uses
four independent clauses — a single-signal implementation would have emitted a false green on
this exact run.

**Mutation controls** (`evidence/25-cloud-mutation-controls.txt`): pristine accepted at 14/14
before and after; dropping the `previous_state`, witness or boot-id clause reddens exactly the
two tests claiming to cover them; dropping the nonce tag reddens exactly its own test; the file
was restored byte-for-byte.

### The provenance gate — why the equivalence diff is not sufficient on its own

The reference task is `cat input.bin`, whose output is byte-identical to its input. **A backend
that echoed the input back produces exactly the digest a genuine run produces**, and `backend
diff` reports EQUIVALENT while nothing ran in the cloud. That is precisely what defect 3 did, so
the equivalence diff could not have caught it and a separate gate was required.

A task reading `/etc/alpine-release` — a file the Ubuntu controller does not have — settles it:

```
guest file, read by an INDEPENDENT instrument (raw API exec, separate machine): "3.20.10\n"
sha256(guest file)            = bf4e4df148b791a0915b623982122ab212b50e0a4b363a8b75fe60e12293ac17
sha256(submitted input)       = 0f7545d631dd44c993749b9ba2f66684427ca7eb761223e1adeccde7713c2370  <- echo digest
cloud receipt artifact sha256 = bf4e4df148b791a0915b623982122ab212b50e0a4b363a8b75fe60e12293ac17
NEGATIVE CONTROL, same task on local: terminal=Failure{exit-1}, artifact=None
```

The cloud artifact is the guest's file, not the echo digest, and the controller cannot perform
the task at all.

---

## Criterion 4 — the cloud orphan half

### Verdict: the **cloud half is MET**. Criterion 4 **overall remains NOT MET** — the ssh orphan surface is still unmeasured, and that is not this lane's mandate.

The cloud surface previously reported `NOT MEASURED`, which was correct behaviour but not a pass.
It now measures, and the measurement is checked by a control in both directions:

| gate | result | exit |
|---|---|---|
| positive control — a real leaked machine | `count 1 (MEASURED)`, row `machine 82d1d97b062338 (aged-paper-9847) state=started` | **1** (nonzero required) |
| negative control — a nonce nothing carries | `count 0 (MEASURED)` | 0 |
| after destroying the leak | `count 0 (MEASURED)` | 0 |
| app emptiness | `GET …/machines` → `[]`, 0 machines | 0 |

**The positive control was not planted for the scanner's benefit.** A helper in this lane's own
provenance script parsed the API response with `tail -1`, which returns the trailing blank line
rather than the JSON body. The machine id came out empty, the destroy call was a no-op, and a
real billable machine was left running. The scanner was then asked to find an orphan nobody had
arranged for it — and found it, with its raw row. The replacement parses the whole response and
**verifies the destroy by re-reading the machine list** rather than trusting the call.

A scan that returns 0 for a leaked machine *and* for an unused nonce is blind. This one
distinguishes them, so the zero is a measurement.

---

## Every machine created, and its disposition

| machine | purpose | disposition |
|---|---|---|
| `873971b0041558` | vendor semantics probe | destroyed, verified `[]` |
| `8ed9d7dc3e5618` | reference-task run | destroyed by the backend |
| `873264c0070738` | provenance-task run | destroyed by the backend |
| `82d1d97b062338` | provenance oracle — **leaked by FINDING 4** | destroyed, verified `[]` |
| `48ed957da55298` | provenance oracle, retry | destroyed, verified `[]` |
| `80e9e51a1d42d8` | suspend-vs-stop control | destroyed, verified `[]` |

**Final state: `wayland-f25-test` holds 0 machines** (HTTP 200, body `[]`). Nothing is billing.

---

## Scope note — app-scoped, not org-scoped

`WAYLAND_F25_CLOUD_ORG` is `sean-donahoe`, Sean's **personal** organization — not the throwaway
the credential probe requested. It is empty today, so an org-wide emptiness assertion would pass
right now, but it will not stay empty and **a future reader must not believe the org was
disposable.**

This lane took the `(or app)` arm the probe's own requirement permits: it created **one app,
`wayland-f25-test`**, and asserts emptiness over **that app's** machine list only. Every live run
sets `WAYLAND_F25_CLOUD_ORG=wayland-f25-test` explicitly.

The credential value was never printed, logged, committed or transmitted. Verified by grepping
every committed artifact for the literal token on the host that holds it: **0 files matched.**

---

## Findings

| # | Severity | State | Finding |
|---|---|---|---|
| 1 | HIGH | FIXED | Machine create sent no body; the leg had never created a machine |
| 2 | HIGH | FIXED | No nonce metadata was set, making the cloud orphan scan a structural false zero |
| 3 | HIGH | FIXED | The task never ran on the machine; the controller echoed the input, which the equivalence diff cannot detect |
| 4 | MEDIUM | FIXED | This lane's own `tail -1` parse leaked a billable machine; became the Criterion 4 positive control |
| 5 | LOW | BACKLOG | `WAYLAND_F25_CLOUD_ORG` holds an **app** slug, not an org slug. Renaming is a cross-lane operator-surface change, out of scope here; documented on the constant |
| 6 | LOW | observation | Guest uptime is the weakest hibernation clause and failed to discriminate on the measured stop control. Retained, but must never be the only signal |

## What this lane did NOT do

- Did **not** re-run the ssh surface at this commit (target absent). Criterion 1's ssh leg rests
  on 25-01's evidence.
- Did **not** measure the **ssh** orphan surface, so Criterion 4 is not closed overall.
- Did **not** touch Criteria 2 or 3, or any status row other than 1 and 4.
- Did **not** exercise cloud **cancellation** live. `cancel` now re-enumerates by a nonce that
  machines actually carry, so the defect-2 class is closed for it, but no live cancel was driven.
- Did **not** rename `WAYLAND_F25_CLOUD_ORG` (FINDING 5).
