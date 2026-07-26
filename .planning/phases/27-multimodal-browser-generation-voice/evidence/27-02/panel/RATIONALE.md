# Decision: `chain-plus-derived-flags`

**Won on: majority — 4-0, and the fourth vote was cast against its own
starting position.**

## What was decided

Publish the activation chain for browser, computer use and web AND redefine
the existing `capabilities.browser_suite` / `capabilities.computer_use`
booleans to mean live readiness, preserving the linkage answer under separate,
explicitly named additive fields.

## The capture that decided it

**OBS-07**, read together with **OBS-01**.

OBS-01 captured the shipped binary on `hetzner-dsm` publishing
`browser_suite=true` and `computer_use=true`. OBS-07 then drove the operation
that flag promises, on that same machine, through the product:

```
is_error=True
session: backend error: Camoufox is unavailable at http://localhost:9377/health
and Core could not start `camofox-browser`: spawn camoufox: No such file or
directory (os error 2).
```

The handshake said available; the very next operation could not deliver. Both
rival options — `activation-chain-only` and `chain-plus-new-flags` — leave
that exact sequence intact for every consumer that exists today. They add an
honest signal beside a live false one, which satisfies the requirement in
letter and not in effect.

**OBS-02..05** removed the alternative explanation. Each introduced ONE
absence — no browser binary resolvable, a display advertised with nothing
behind it, cloud credentials absent, cloud credentials present — and the
capability claim was invariant across all five captures. The flags are not a
degraded probe; they are not a probe at all.

**OBS-06** established that this is a wiring gap and not a missing mechanism.
The activation ladder publishes honest `unavailable` with a reason for
`pricing_refresher` (`disabled_by_config`), `learned_policy`
(`runtime_path_unwired`), `smart_handoff` and `delegate_isolation`, and a full
`declared → configured → constructed → ready` chain for the two capabilities
that genuinely are ready. It publishes nothing at all for browser, computer
use or web.

## Why the known cost was accepted

Redefining the meaning of a field already on the wire is a compatibility
event. That is stated plainly and is carried in the contract bump.

The measurable half of that cost is that `crates/wcore-cli/tests/
release_binary_smoke.rs` goes RED, because it asserts `browser_suite == true`
and `computer_use == true`. All four members counted that cost and all four
reached the same reading of it: on the machine CI actually runs on, that test
asserts a statement OBS-01..07 measured to be false about that machine, and it
passes only because the flag does not mean what its name says. Its red under a
truthful redefinition is not a break being detected — it is a defect no longer
being hidden.

## Conditions carried with the decision

Both come from the internal adversarial pass and are binding on the
implementation:

1. **The LTO guard must be retargeted, not deleted.** `release_binary_smoke.rs`
   exists to catch a release-profile dead-code-strip regression (v0.2.0
   BLOCKER #1) that stripped `inventory::submit!` items. That guard must move
   verbatim onto the new additive linkage field. If it is dropped instead of
   moved, this decision trades one defect for another.
2. **The Desktop consumer's actual reading of these flags is UNVERIFIED from
   this repository.** Three members reasoned that Desktop already consumes the
   booleans as readiness, which would make the redefinition a bug fix for it.
   That claim is not measurable here and was not measured. It is recorded as
   the open risk this decision carries, and must be confirmed before the
   contract bump is published to a host.

## What this decision did NOT settle

`OBS-09` — the browser tool's remediation text instructs `[browser]
allowed_origins`, while the key the config actually reads is
`[browser.policy] allowed_origins`. Following the printed instruction verbatim
leaves the tool disabled. Two members (codex, kimi) independently required
this be fixed in the same work. It is a separate defect from the publication
question and is recorded in the audit, not decided here.
