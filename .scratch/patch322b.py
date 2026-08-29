p = "crates/wcore-cli/src/tui/commands/at_ref_resolve.rs"
s = open(p).read()

old = """            // core#322 c4: a VCS control directory or content store is never
            // useful context, can be enormous, and reconstructs committed
            // secrets through its own porcelain. This was a literal `.git`
            // NAME test, which missed `.hg`/`.svn`/`.bzr` outright and missed
            // a `.git` reached under any other name. The shape test is the one
            // `wcore-tools`' deny walk uses — one list, one owner — and it is
            // asked about the RESOLVED path, so the entry's own name is
            // irrelevant.
            if wcore_tools::workspace_policy::is_vcs_store_or_control_dir(&canonical) {
                continue;
            }"""
new = """            // core#322 c4: a VCS control directory or content store is never
            // useful context, can be enormous, and reconstructs committed
            // secrets through its own porcelain. This was a literal `.git`
            // NAME test, which missed `.hg`/`.svn`/`.bzr` outright and missed
            // a `.git` reached under any other name. The shape test is the one
            // `wcore-tools`' deny walk uses — one list, one owner — and it is
            // asked about the RESOLVED path, so the entry's own name is
            // irrelevant.
            //
            // The predicate tests the path AND its ancestors, because pruning
            // alone does not cover this walk: a symlink aimed BELOW a store
            // root is met at the top of the tree and never descended TO, so a
            // self-only test admitted `.git/objects/aa` and inlined the
            // objects under it.
            if wcore_tools::workspace_policy::is_within_vcs_store_or_control_dir(&canonical) {
                continue;
            }"""
assert old in s, "dir arm anchor miss"
s = s.replace(old, new, 1)

old2 = """            if !admitted.canonical.starts_with(root_canonical)
                || is_secret_path(&path)
                || is_secret_path(&admitted.canonical)
            {
                *skipped += 1;
                continue;
            }"""
new2 = """            if !admitted.canonical.starts_with(root_canonical)
                || is_secret_path(&path)
                || is_secret_path(&admitted.canonical)
                // core#322 c4: the same reach on the FILE arm. `is_secret_path`
                // matches secret NAMES and an object file is named after its
                // hash, so a link straight at `.git/objects/aa/deadbeef` was
                // read and inlined without any store predicate being consulted.
                || wcore_tools::workspace_policy::is_within_vcs_store_or_control_dir(
                    &admitted.canonical,
                )
            {
                *skipped += 1;
                continue;
            }"""
assert old2 in s, "file arm anchor miss"
s = s.replace(old2, new2, 1)
open(p, "w").write(s)
print("walk patched")
