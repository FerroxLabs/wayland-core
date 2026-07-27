PANEL-MEMBER: internal (adversarial)
---
# Internal adversarial pass — argued AGAINST the emerging consensus

The consensus heading into this round is `contract-holds`. My job is to attack it, so here is
the strongest case for `contract-cosmetic` that I can build, and then my honest verdict.

## The case against `contract-holds`

1. **Every field is still a `String`.** `CredentialRef` has two of them and `DiscoveredItem`
   has four plus a map. Rust cannot stop a producer writing a secret into any `String`. On a
   strict reading, "unrepresentable" is unachievable for any type that contains a `String`,
   and therefore the contract as literally worded can never be satisfied by anything short of
   an enum over fixed literals. If that is the standard, the honest verdict is
   `contract-cosmetic` forever, for this and every future revision.

2. **The scrubbers are heuristics.** `scrub_detail` knows about URL userinfo, secret-named
   query parameters and secret-named flags. It does not know about a credential passed
   positionally (`runner SECRET`), base64 blobs in a path segment, or a token in an MCP
   `headers` map — and `headers` is carried on `McpServerConfig` even though this projection
   does not currently emit it. A future edit that starts emitting `headers` would reopen the
   channel and no existing test would notice.

3. **The probe is corpus-shaped.** It asserts that canaries FROM THE MANIFEST are absent. It
   is strong evidence about the shapes present in the corpus and silent about shapes that are
   not. Absence of evidence, etc.

## Why I nonetheless vote `contract-holds`

Point 1 proves too much. Under it no verdict other than `contract-cosmetic` is ever reachable,
which makes the option set decorative — and an option that cannot lose is the same defect as a
gate that cannot fail. The plan's own wording resolves this: the question is whether the
emitted type has a field WHOSE PURPOSE is to carry a value, or whether a value can only arrive
by a producer misusing a field reserved for a location. After three rounds of closure, the
answer is the latter: `credential` is a two-field location record whose constructor takes no
value parameter and whose name is narrowed to an identifier shape on every path including
deserialization; `details` is private, scrubbed by its only writer and scrubbed again on
deserialization; and the internal `MigrationPlan` that genuinely does hold an `api_key` has no
conversion INTO the emitted type that reads it.

Points 2 and 3 are real and I am recording them as residual risk rather than as a refutation:
they describe shapes that could reopen the channel under FUTURE edits, not a path by which a
value travels today. The right response is the one already taken — the probe is the standing
guard, and it is load-bearing rather than decorative. I am carrying `headers` forward as a
named follow-up rather than pretending it is covered.

Decisive for me: three rounds where a member named a concrete path, the path was CLOSED in
code and re-measured, rather than argued away. That is the procedure working.

PANEL-VERDICT: contract-holds
PANEL-BASIS: A value can now reach the emitted plan only by a producer misusing a field reserved for a location, every such field having been narrowed or made private with scrubbing on all construction paths, so the redaction is enforced by the type rather than by any printer.
