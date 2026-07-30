# Decision: degrade, not refuse — and why, including the case against

## The choice

On a host with **no confidential-capable credential store at all** (no usable OS
keyring AND no unlocked vault), durable session persistence is turned **off** at
config resolution, and the operator is told once, at startup, on stderr.

Two neighbouring situations deliberately keep today's **hard refusal**:

| situation | disposition | why |
|---|---|---|
| no keyring **and** no vault | **degrade** + notice | the host cannot do it; the operator did not ask for it |
| `backend = "plaintext"` | refuse (unchanged) | the operator configured a backend that can never hold confidential material — a real misconfiguration, and there is already a test asserting the refusal reaches the Desktop host |
| vault passphrase present but wrong | refuse (unchanged) | the operator clearly wants a vault; silently running without one would hide it. Falls out for free: the store *opens*, so the wrong key surfaces later as the distinct `SecureStoreUnreadable` |

## Cross-audit panel (LANE-BRIEF §4)

| leg | position | substance |
|---|---|---|
| codex `gpt-5.6-sol` | **DEGRADE** | "no confidentiality property is weakened — with no recovery journal the encrypted provider request is not written to disk at all. What is lost is durability, not secrecy." Rider: never report plain `Healthy`. |
| kimi K3 | **DEGRADE** | "crash recovery is availability, not correctness … you cannot leak what you never persist." Riders: health must report it; consider treating an *explicit* `enabled = true` more harshly than the default. |
| gemini 3.1 Pro | **REFUSE** | "a stderr notice in a headless environment is easily missed in logs, leading to silent data loss when the operator attempts to resume a session. Require an explicit opt-out." |
| internal adversarial | see below | |

**Vote: DEGRADE 2 — REFUSE 1.** I take the majority, and I take one rider from
the minority seriously enough to have built for it (below).

**Two legs were nearly lost to instrument defects, both of the class LANE-BRIEF
§4 names.** `gemini` and `kimi` both returned **0 bytes with rc=0** on the first
attempt — a silently dropped vote that a `PANEL_POSITION=` grep would have
scored as "no answer" rather than "broken harness". Both answered on a shorter
prompt. `codex` returned **39 bytes**: `Reading additional input from stdin…` —
it blocks for a TTY the agent shell does not provide, and needed `< /dev/null`.
Every leg was verified alive on a real question before its vote was counted.

## The case against my own choice, taken seriously

The strongest argument for REFUSE, which gemini gets right and which I want on
the record rather than waved away:

> Under degrade, a gateway operator's bot silently forgets every conversation
> across restarts. The only trace is one stderr line at startup, which in a
> `systemd` unit scrolls into the journal and is never read again. The operator
> then experiences "the bot has amnesia" as a *mystery*, with the explanation
> buried hours earlier in a log. That is structurally the same failure being
> fixed: degraded behaviour whose diagnostic lives somewhere the operator is not
> looking.

That is a real cost and it is not zero. Four things bound it:

1. **The notice is not buried at the point of emission.** Measured, not assumed:
   on `gateway run` it is printed *before the gateway's own first line*
   (`[gateway] channels registered=0`), and on a CLI run it is the second line
   of output, ahead of the entire INFO wall. See
   `live-gw1-postfix-novault.txt`.
2. **It names the consequence, not just the cause** — "conversation history is
   not saved to disk and an interrupted turn cannot be recovered" — so the
   amnesia is predicted in the same sentence that explains it.
3. **It reprints on every process start.** A `Restart=always` service re-emits it
   each restart; it is not a one-time-ever banner.
4. **The alternative is the release blocker in a different shape.** Refuse-to-start
   makes the default install on *every Linux server* dead until the operator
   discovers an env var or a new opt-out flag. The brief is explicit that "a
   default that works on a server is worth more than a documented workaround",
   and REFUSE cannot satisfy that by construction.

**What I conceded to the dissent.** Both DEGRADE legs independently added the
same rider, and it is the same weakness gemini's REFUSE argument rests on: the
degraded state must be *reportable*, not merely *printed once*. So
`wcore_config::config::durable_sessions_disabled_by_host()` now exists, because
`session.enabled == false` cannot distinguish "the operator asked" from "the host
forced it", and those two want opposite reporting. **It has no consumer in this
change** — the surface that should read it is `channel health`, which belongs to
`lane/fix-channel-health-truth`. That is a deliberate handoff, not an oversight,
and it is stated as unfinished in the lane report.

**What I rejected from the dissent.** kimi and codex both suggested treating an
*explicitly configured* `enabled = true` more harshly than the default-on case.
`session.enabled` has no per-key provenance after config merge, so telling the
two apart needs machinery that does not exist; and the operator it would punish
is the one already holding the release blocker. Uniform degrade plus a loud
notice, with the dissent recorded here.

## Is any security property weakened?

**No, and all three panel legs agree independently.** The confidential key exists
so the crash-recovery journal never holds provider requests in cleartext. With
durable sessions off, **no journal is written at all** — there is nothing at rest
for that encryption to protect. Confidentiality at rest is monotonically better
in the degraded mode, not worse. What is lost is durability: saved history and
recovery of an interrupted turn.

Measured, not argued: the pre-fix keyring-less run created
`sessions/<id>.journal` and *then* died; the post-fix run creates **no
`sessions/` directory at all** (`live-q2-…` vs `live-q1-…`, `SESSION_FILES=0`
both times but with a materially different file listing).

The one refusal that is genuinely a security property — plaintext credentials
cannot hold confidential material — is **untouched**, and that is proven rather
than claimed: `json_stream_startup_refusal::plaintext_credentials_refusal_reaches_the_host`
still passes, 6/6 in that file.
