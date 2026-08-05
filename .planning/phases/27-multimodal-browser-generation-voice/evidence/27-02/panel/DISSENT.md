# Dissent — every losing option, its strongest argument, and what would have won it

The external panel was 3-0 for `chain-plus-derived-flags` and the internal
adversarial pass was written specifically to break that. It failed, and its
failure is recorded below alongside the arguments it made, because an
unrecorded losing argument cannot be re-examined when the risk it named
materialises.

## `chain-plus-new-flags` — the strongest rival, argued at full strength

**Best argument made for it.** Redefining a live field is a class of change
with no remedy once a host has shipped against it: you cannot un-ring it, and
you cannot tell from the wire which meaning a given peer holds. Adding a
distinctly named second boolean has a known remedy for its stated
con — the "subtle difference" hazard is a naming problem, and naming problems
are fixable. The safety asymmetry therefore runs the opposite way from how the
majority stated it: under new flags the worst case is a host that keeps its
current already-shipped behavior (an unfixed defect, recoverable next
release); under redefined flags the worst case is a host that silently changes
behavior on an engine upgrade with no error, no event and nothing in its own
logs to explain it.

**Evidence that would have made it win.** A captured demonstration that any
real consumer reads `browser_suite` as *linkage* and acts on that reading —
for instance a Desktop code path that uses the flag to decide whether to offer
an install prompt rather than whether to offer the feature. No such capture
exists. The only in-repo consumer, `release_binary_smoke.rs`, reads it as
linkage but is itself the artifact under repair.

**Why it lost.** Its measured witness turned on it. It cited
`release_binary_smoke.rs` going red as proof the change breaks the only
observable consumer — but OBS-01..07 show that test asserting a statement that
is false about the machine it runs on, passing only because the flag does not
mean what its name says. A witness whose testimony is the defect cannot be
cited against the repair.

## `activation-chain-only`

**Best argument made for it.** Zero wire delta, and OBS-06 proves the ladder
already works, already carries a fixed reason vocabulary and already refuses to
construct a reason-less unavailable. Every other option adds surface to a
protocol that already has the right mechanism and has simply not been pointed
at these three capabilities.

**Evidence that would have made it win.** A demonstration that the Desktop
host already consumes `capability_activation` events. If it does, the booleans
are legacy noise and touching them buys nothing at real cost.

**Why it lost.** It leaves OBS-07's false ready fully reachable by every
consumer that exists today. Kimi put it most sharply: the honest signal would
sit next to a live lie, and the host this repository cannot patch keeps
believing the lie.

## `escalate`

**Best argument made for it.** The decision turns on the behavior of a
consumer in a repository Sean owns and could read in about ninety seconds.
Asking that question is not parking a decision — it is asking the one question
that decides it.

**Evidence that would have made it win.** A genuine 2-2 split with no captured
experiment separating the positions. There was none: the panel was 4-0.

**Why it lost.** The phase rule reserves escalation for a real deadlock, and
the asymmetry Sean would have been asked to resolve does not exist. One cost
is a declared, revertible, contract-announced semantics change; the other is a
handshake that keeps claiming capabilities the machine cannot deliver.

## The risk this decision knowingly carries

The internal pass's fifth argument was NOT withdrawn and is not answered by
anything in the record: **the Desktop consumer's actual reading of these flags
is unverified from this repository.** Three of four members reasoned from an
assumption about it. That assumption is load-bearing for the claim that the
redefinition is a fix rather than a break, and it was not measured. It is
carried into `RATIONALE.md` as a binding pre-publication condition rather than
treated as settled.
