# The hostile corpora are NOT committed here, and that is the point

This directory holds no fixture tree. It holds the reason there isn't one.

`scripts/portability-hostile-gen.py` materialises every adversarial corpus on
the target platform **at run time**, from a declarative spec. Committing the
corpora would destroy the property most of them exist to test:

- **macOS** normalises filenames and is case-insensitive by default. Two peer
  items named `Collide` and `collide`, or `café` in NFC and `café` in NFD, are
  **one file** after a checkout on APFS.
- **Windows/NTFS** is case-insensitive (though *not* normalisation-insensitive,
  which is a real and measured difference from APFS).
- **Linux** is neither, which is why it is the authoritative proof host.

So a committed corpus is a corpus whose most interesting distinction was already
collapsed by whichever filesystem last checked it out — and the suite that reads
it goes green having tested one file while claiming to test two. The generator
therefore verifies **after** creation that each declared distinction actually
survived, records `collapsed: true|false` per case in `cases.json`, and exits
non-zero when the distinction collapsed on a platform where the case declared it
must hold.

Measured on this program's own hardware, 2026-07-28:

| Case | Linux (hetzner-dsm) | macOS (Sean's Mac, APFS) | Windows (SeanDesktop, NTFS) |
|---|---|---|---|
| `conflict-casefold` | distinct | **collapsed** | **collapsed** |
| `conflict-normalform` | distinct | **collapsed** | distinct |

## Conventions this follows

`crates/wcore-fixture-harness/src/lib.rs` establishes the archetype model that
`crates/wcore-cli/tests/portability_hostile_corpus.rs` follows: a sanitised
snapshot of a `$WAYLAND_HOME`, the engine binary **spawned** against it, and
assertions on the emitted document, on stderr cleanliness and on a post-run
state diff. That crate's catalog, playback and replay are a Wave 1 skeleton and
are **not built**, so this work follows its convention without calling an API it
does not have.

Its sanitisation rule is honoured absolutely and is non-negotiable: **no fixture
file may contain a real API key, a real personal email, or a real machine path.**
Every secret in every hostile corpus is a synthetic canary of the form
`wlc-hostile-canary-<n>-DO-NOT-USE`, and no real peer home (`~/.hermes`,
`~/.openclaw`) is read, copied or referenced by any file in this plan.

## Declared outcomes

Each case carries its expected outcome as **data**, because a hostile case whose
only assertion is that the process exited passes when the product silently does
the wrong thing. The four legitimate outcomes are `imported`, `quarantined`,
`refused` and `conflict`; a case declaring anything else fails
`hostile_every_case_declares_a_legitimate_outcome_and_what_it_attacks`.
