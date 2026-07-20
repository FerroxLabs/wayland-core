---
phase: 20-transactional-delegated-mutation
plan: "06"
subsystem: swarm
tags: [delegated-mutation, candidate-seal, isolated-checkout, opaque-capability, source-packet]
requires: ["20-01", "20-02", "20-03", "20-04"]
provides:
  - Opaque, live, non-serializable, non-cloneable CandidateSeal minted only by the accepted 20-03 live standalone-checkout capability
  - Files-only before-and-after revalidation via the retained directory authority (no git subprocess), treating .git as attacker-controlled
  - A SHA-256 source-manifest digest binding the full git-tree identity (path + owner-exec mode + content), with adversary-resistant .git inspection
affects: [20-09]
tech-stack:
  added: [sha2]
  patterns: [opaque live capability seal, retained-authority filesystem revalidation, deny-by-default git-config allowlist, SHA-256 git-tree-identity manifest, fail-closed plumbing inspection]
key-files:
  created:
    - crates/wcore-swarm/src/worktree/candidate.rs
  modified:
    - crates/wcore-sandbox/src/directory_authority_file.rs
    - crates/wcore-swarm/Cargo.toml
    - crates/wcore-swarm/src/worktree.rs
    - crates/wcore-swarm/src/worktree_tests.rs
key-decisions:
  - "The seal is minted only by TransactionWorkspace::seal_candidate, itself reachable only from WorktreeManager::create_isolated_checkout — so only the accepted 20-03 standalone-checkout capability can mint a CandidateSeal."
  - "CandidateSeal is opaque: all fields private, sole pub(super) mint taking references to the already-retained authorities (no caller hash/path/serialized field), no serde, no Clone/Copy, redacted Debug."
  - "Creation and revalidation inspect Git plumbing strictly as files through the retained wcore_sandbox::DirectoryAuthority (relative O_NOFOLLOW opens) — never by executing git and never by shelling out."
  - "The delegated worker controls the checkout, so .git is treated as attacker-influenced: revalidate rejects a .git/commondir redirect, the presence of .git/config.worktree, and any .git/config outside a deny-by-default benign core/branch/extensions allowlist — closing relocation/command vectors (core.hooksPath, core.fsmonitor, core.sshCommand, core.gitProxy, filters, includes, aliases, remotes, credential/url/protocol, extensions.worktreeConfig)."
  - "The source-manifest digest is SHA-256 (sha2), domain-separated and length-framed, binding the full git-tree identity: entry path + the owner-exec bit (git's canon_mode: mode & 0o100, distinguishing 100644 from 100755) + content, with top-level .git excluded — so type swaps, renames, empty-dir add/remove, content changes, and executable-bit flips all perturb it."
  - "Tracked working-tree symlinks are not yet bindable (no no-follow readlink primitive on the retained authority; adding one is out of scope): a symlinked entry fails closed with a specific 'does not support tracked symlinks' diagnostic. Binding symlink targets is deferred to the consuming landing plan."
  - "The seal fails closed on a released transaction and an outstanding checkout descriptor loan, so it grants no authority once its checkout is (or is being) torn down and can never outlive the checkout it binds."
patterns-established:
  - "A landing candidate is proven by an opaque live capability produced only against a checkout this process currently holds authority over, re-deriving the full git-tree identity from the current filesystem — with .git treated as attacker-controlled — before it is trusted."
requirements-completed: []
duration: n/a
completed: 2026-07-20
status: complete
source_sha: 10d75737a42b0d6b9aeaa42f1dea9fb06e5613c7
source_tree: a678cb30d0e8b96cb952fe21aed0118a184b9a4b
task_base: c0ac6721e309e000f6090d76b4a545ff20228861
changed-paths:
  - crates/wcore-sandbox/src/directory_authority_file.rs
  - crates/wcore-swarm/Cargo.toml
  - crates/wcore-swarm/src/worktree.rs
  - crates/wcore-swarm/src/worktree/candidate.rs
  - crates/wcore-swarm/src/worktree_tests.rs
coverage:
  - id: SC1
    description: "Only the accepted 20-03 live standalone-checkout capability can mint a non-serializable, non-cloneable CandidateSeal."
    verification:
      - kind: unit
        ref: "remote-cargo.sh f20-06-swarm-t ... test -p wcore-swarm (92 lib passed; candidate_seal_mints_and_revalidates_from_fresh_checkout, candidate_seal_cannot_outlive_its_checkout)"
        status: pass
  - id: SC2
    description: "The seal binds the full git-tree identity and .git isolation, and is revalidated before and after with .git treated as attacker-controlled."
    verification:
      - kind: unit
        ref: "remote-cargo.sh f20-06-swarm-t ... test -p wcore-swarm (drift/substitution/alternates + commondir/worktree-config/deny-by-default-config/hooksPath/fsmonitor/gitProxy rejection, manifest boundary + owner-exec-bit tests, tracked-symlink diagnostic)"
        status: pass
  - id: SC3
    description: "Strict Clippy and the wcore-swarm + wcore-sandbox test suites pass remotely on Linux."
    verification:
      - kind: other
        ref: "remote-cargo.sh f20-06-swarm ... clippy -p wcore-sandbox -p wcore-swarm --all-targets --all-features -- -D warnings (Finished clean); test -p wcore-sandbox (60 passed)"
        status: pass
---

# Phase 20 Plan 06: Opaque Live CandidateSeal (20-06A source packet)

**An isolated delegated-mutation checkout can now mint an opaque, live `CandidateSeal` — producible only by the accepted 20-03 standalone-checkout capability, binding the transaction/object identity, pinned base/head/tree, and a SHA-256 digest of the full git-tree identity (path + owner-exec mode + content), and re-derived from the current filesystem (files only, no git subprocess, `.git` treated as attacker-controlled) before it is trusted. Proven on Linux.**

## Scope

Five source files (task base `c0ac672`, source `10d7573`):

- `crates/wcore-swarm/src/worktree/candidate.rs` — new: the opaque `CandidateSeal`, its sole `pub(super)` mint, `revalidate`, the deny-by-default `scan_config`, and the SHA-256 manifest walk.
- `crates/wcore-swarm/src/worktree.rs` — `TransactionWorkspace::seal_candidate` (sole caller-facing entry) + `#[path]` module wiring.
- `crates/wcore-swarm/src/worktree_tests.rs` — 21 crate-private failure-injection + boundary tests.
- `crates/wcore-swarm/Cargo.toml` — `sha2` (workspace dep) for the manifest digest.
- `crates/wcore-sandbox/src/directory_authority_file.rs` — `RegularFileAuthority::is_executable` (owner-exec bit from the retained fd), for git-tree-mode binding.

## Adversary-resistant `.git` inspection + full-tree binding

Because the delegated worker controls the checkout, `revalidate` treats `.git` as hostile and — beyond base/head/tree, identity, loan, and release checks — rejects a `.git/commondir` redirect, the presence of `.git/config.worktree`, and any `.git/config` outside a deny-by-default benign `core`/`branch`/`extensions` allowlist (closing `extensions.worktreeConfig`, `core.hooksPath`/`fsmonitor`/`sshCommand`/`gitProxy`/pager/editor/askpass, filters, includes, aliases, remotes, credential/url/protocol). Object aliasing (alternates, loose+packed replace refs), linked-worktree metadata, planted hooks, and symlink escape remain rejected. The manifest digest binds the full git-tree identity — path + git's owner-exec bit (`mode & 0o100`) + content — so a bare post-mint `chmod` that flips git's tree mode is detected as drift.

## Verification (Linux, committed-head Hetzner harness, source `10d7573`)

- **`clippy -p wcore-sandbox -p wcore-swarm --all-targets --all-features -- -D warnings`:** Finished clean.
- **`test -p wcore-swarm`:** 92 lib tests passed, 0 failed; all integration binaries passed. **`test -p wcore-sandbox`:** 60 passed, 0 failed.
- **21 CandidateSeal tests, all green**, including the adversary-`.git` hardening tests (commondir, worktree-scoped config, `extensions.worktreeConfig`, relocated `core.hooksPath` with a planted executable, `core.fsmonitor` program, `core.gitProxy`, deny-by-default config scan), the manifest boundary cases (empty-dir add/remove, file↔dir swap, `.git` exclusion), the git owner-exec-bit binding (`0o644`↔`0o645` ignored, `0o645`↔`0o745` detected), and the specific tracked-symlink diagnostic.
- **Per-task construction gates:** `vx cargo fmt` clean; `git diff --check` clean; `verify-task-scope.sh … paths=5`.

## Review-driven hardening (three independent-review rounds)

The initial 20-06A source was prosecuted by the 20-09 independent review across three rounds, each catching real gaps repaired at source and re-proven on Linux before the review was allowed to pass: (1) three HIGH false-PASSes in `.git` inspection — a `.git/commondir` redirect, worktree-scoped config, and command-execution config directives (`core.hooksPath`/`fsmonitor`) bypassing the seal's own hook scan — plus a tracked-symlink false-FAIL and a weak 64-bit digest; (2) an unbound file mode (a `chmod +x` escaping drift detection) and a `core.gitProxy` denylist gap (fixed by binding the exec bit and inverting the core scan to a deny-by-default allowlist); (3) an exec-bit mask that over-counted group/other bits, corrected to git's owner-exec canonicalization (`mode & 0o100`). This SUMMARY records the hardened successor `10d7573`.

## Explicit non-claims

This source packet makes **no** receipt, containment, acceptance, landing, or lifecycle claim, and marks no F20 requirement complete. It establishes only an opaque, live, exactly-revalidated candidate identity. No signing, generic-evidence, receipt, containment, acceptance, CAS, or landing infrastructure was created. No compilation or runtime claim is made on macOS.

## Readiness

Ready for the distinct non-author review plan **20-09** (profile `f20-09`), which independently reviewed this hardened 20-06A candidate-seal source packet at source `10d7573`.

---
*Phase: 20-transactional-delegated-mutation*
*Completed: 2026-07-20*
