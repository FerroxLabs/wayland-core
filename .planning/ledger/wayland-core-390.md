---
issue: 390
repo: FerroxLabs/wayland-core
kind: defect
title: "is_vcs_content_store arm 2 reads only <root>/.git, so a VENDORED gitfile's object store is VFS-readable (split from #244 c1)"
status: open
last_verified_commit: a278f8c3
criteria:
  - id: c1
    text: "A VFS read of an object under a store named by a gitfile on a VENDORED checkout is refused, with that checkout`s own working tree still readable as the wrong-refusal control"
    state: not-met
    owner: core
    note: "MEASURED ABSENT 2026-08-30 on hetzner, by the fixture that closes #244 c3`s second arm rather than by inspection. Fixture: `<root>/vendor/pkg/.git` is the FILE `gitdir: ../pkg-git`, the real object lives at `<root>/vendor/pkg-git/objects/12/3456`. `WorkspacePolicy::contained(&root).is_vcs_content_store(&store_file)` returns FALSE -- asserted in-tree at grep_vcs_named_store_deny.rs::grep_cannot_harvest_a_nested_gitfile_named_store, which PASSES -- so SecretDenyFs::guard admits those bytes and every VFS method that routes through it (read, read_pinned, exists, list, metadata, ...) returns them. Arm 1 cannot see the path lexically (`pkg-git/objects` is not a `(control, store)` pair) and arm 2 reads `<root>/.git` only, so it never looks. GREP IS ALREADY CLOSED for this case and that is not this criterion: grep_policy resolves vcs_content_stores(dir) for every directory it traverses, which makes Grep deliberately STRICTER than the point-predicate here."
  - id: c2
    text: "The same holds for an objects/info/alternates borrow declared by a NESTED checkout, not only by the workspace root"
    state: not-met
    owner: core
    note: "Not separately measured; it is the same miss by the same mechanism -- alternate_object_dirs is only ever called on `<root>/.git/objects` and on the dirs a ROOT gitfile names. Stated as its own criterion rather than folded into c1 because a fix that special-cases gitfiles would close c1 and leave this open, and that is exactly the shape of partial this ledger exists to refuse."
  - id: c3
    text: "Whatever caching the fix introduces is measured against #376`s complaint: the per-operation cost of is_vcs_content_store does not get worse than it is today, stated as a number"
    state: not-met
    owner: core
    note: "This is why the fix was NOT attempted in the change that found it. Extending arm 2 to nested control dirs needs DISCOVERY -- finding every `.git` gitfile under the root -- inside a per-path predicate on the hot SecretDenyFs::guard path. #376 already reports that rebuilding this list on every ordinary path operation is a cost problem at the current ROOT-ONLY scope; a tree walk per call is strictly worse. Grep can afford the walk because it traverses once per invocation and reuses the result for that invocation; the VFS needs a different shape (resolve-on-first-touch cache, or upward detection from the queried path), which is a design decision with a measurable cost. A fix that closes c1 and c2 while making #376 worse is not an improvement, so the number is a criterion and not an afterthought."
  - id: c4
    text: "When the fix lands, grep_vcs_named_store_deny.rs`s `!is_vcs_content_store` assertion is INVERTED rather than deleted, so the two layers are re-tied"
    state: not-met
    owner: core
    note: "The assertion exists today as a deliberate record of the divergence between what Grep denies and what the point-predicate sees, not as an endorsement of it. Deleting it when the predicate grows a nested arm would silently drop the only in-tree statement that the two layers are supposed to agree -- which is the drift #244 was filed about in the first place."
---

Split out of #244 c1 while closing #244 c3. #244 c1's text was the unqualified
"at any nested depth"; that has been rewritten to the scope that actually holds
and this issue carries the remainder, referenced from #244 c7.

Found by building the second-arm fixture for #244 c3, not reported by that
change's verifier. The Grep half of the same class IS closed in that change --
it is only the in-process VFS predicate that still misses this shape.
