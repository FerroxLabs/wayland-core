# 25-02 — Decision: what the plugin approval gate enforces by default

**Decided, not parked.** Cross-audited on the 4-way panel, committed, evidence retained.
Verbatim captures: `evidence/25-02-panel-{codex,gemini,kimi}.txt`.

---

## The question

`plugin approve` had to become a real gate: an installed-but-unapproved plugin must not
execute. The open question was **what the default enforcement scope is**, given that existing
installs already have plugin directories in the plugins root that were never approved (the
concept did not exist), and existing integration tests drop plugin dirs into a temp root and
expect them to load.

Four options were put to the panel, with `(Recommended)` stripped so no member could simply
echo a prior:

| | Option |
|---|---|
| **A** | **Root-scoped governance.** A plugins root becomes governed the moment lifecycle state is written into it. Inside a governed root, every plugin needs an approval matching its current digest. An ungoverned root behaves exactly as before. |
| B | Per-plugin governance. Only plugins installed through the new lifecycle need approval. |
| C | Global enforcement, config flag default ON to disable. |
| D | Global enforcement, default OFF. |

## The panel

| Member | Position | Core argument |
|---|---|---|
| Gemini 3.1 Pro | **A** | B is a fatal fail-open — an attacker just avoids the installer. C and D are a false dichotomy between "break the world" and "secure nobody". |
| Kimi K3 | **A** | Implicit sibling gating is a feature, not a defect: installing one plugin through the governed path while unvetted siblings keep executing would be a false sense of security. C's escape hatch is what frustrated operators flip, so C converges to D in practice. |
| Codex 5.6 Sol | **C** | Only C makes authorization universal — filesystem presence alone never permits execution. **A remains fail-open on every ungoverned root.** Upgrade breakage is honest and visible. |
| Internal adversarial (arguing against A) | **sustained one attack** | Under A, an attacker who can write to the plugins root can also *delete* `generations.json` and thereby un-govern the whole root in one file operation. Neither majority member raised this. |

**Decision: A. Basis = majority (2 of 3 external), with the dissent converted into a binding
condition rather than discarded.**

## Why A over C, stated plainly

C is the stronger control in the abstract and I am not pretending otherwise. It loses on two
grounds that are specific to this codebase rather than general:

1. **C's cost is a breaking change to every existing install plus a rewrite of the existing
   on-disk plugin test corpus.** This plan's own rules forbid weakening a test to reach green;
   rewriting 116 passing tests to satisfy a new default is the same move wearing a different
   hat, and it is a scope this fenced plan cannot honestly absorb.
2. **The residual A leaves open is outside this gate's threat model.** The register for this
   plan names four boundaries — marketplace→install root, bundle→signature, install root→loader,
   operator→approval record. All four are about *content arriving through the install path*. An
   attacker with arbitrary local write to `~/.wayland/plugins/` already has power over the user
   account equal to or greater than what the gate protects, and that exposure is identical to
   the pre-existing baseline. A does not make it worse; C would make it better, and that
   improvement is real but is a different piece of work.

Codex's argument is recorded here because it is correct on its own terms, and because whoever
revisits this should start from it rather than rediscover it.

## Binding condition taken from the dissent

Codex's concrete attack — one file deletion un-governs a root — is closed:

> `is_governed()` returns true if **either** `generations.json` **or** `approvals.json` is
> present.

Deleting one marker no longer reverts a governed root to fail-open; un-governing now requires
destroying the approval record itself, which is a loud act rather than a quiet one. Cost: one
boolean `||`. Pinned by
`plugin_governance::tests::deleting_one_governance_marker_does_not_un_govern_the_root`, which
fails against the single-marker implementation.

## What is left open

- `[LOW→MED]` The A residual: an attacker with arbitrary write access to the plugins root can
  delete **both** markers and revert the root to ungoverned. Closing it needs the governance
  marker to live outside the plugins root (e.g. in the profile home) or option C's universal
  enforcement plus a migration story. Filed for whoever owns the plugin trust model; not
  closed here, and not claimed as closed.
