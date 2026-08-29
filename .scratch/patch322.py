p = "crates/wcore-tools/src/workspace_policy.rs"
s = open(p).read()

old = """/// True when `path` IS a VCS content store, or the control directory that owns
/// one (`.git`, `.hg`, `.svn`, `.bzr`), at ANY depth.
///"""
new = """/// True when `path` IS a VCS content store or lives INSIDE one, or is the
/// control directory that owns one (`.git`, `.hg`, `.svn`, `.bzr`), at ANY
/// depth.
///"""
assert old in s, "doc anchor miss"
s = s.replace(old, new, 1)

old2 = """/// Purely lexical, like [`is_vcs_store_dir`]; the caller is responsible for
/// handing it an already-resolved path, which is what makes a store reached
/// under another name answer the same as one reached by its own.
pub fn is_vcs_store_or_control_dir(path: &Path) -> bool {
    if is_vcs_store_dir(path) {
        return true;
    }"""
new2 = """/// The store half is [`inside_vcs_store`] — the SAME predicate the deny walk
/// asks through [`WorkspacePolicy::is_vcs_content_store`] — and not the
/// self-only [`is_vcs_store_dir`]. The two are NOT equivalent and the
/// difference is reachable: pruning governs only what the walk DESCENDS to,
/// while a symlink is an entry met at the top of the tree, so one aimed BELOW
/// a store root (`.git/objects/aa`, neither a `(control, store)` shape nor a
/// control-directory leaf) escaped the self-only test and every object under
/// it was inlined. Graded by
/// `at_dir_prunes_a_path_that_resolves_below_a_store_root`.
///
/// Purely lexical, like [`is_vcs_store_dir`]; the caller is responsible for
/// handing it an already-resolved path, which is what makes a store reached
/// under another name answer the same as one reached by its own.
pub fn is_within_vcs_store_or_control_dir(path: &Path) -> bool {
    if inside_vcs_store(path) {
        return true;
    }"""
assert old2 in s, "fn anchor miss"
s = s.replace(old2, new2, 1)
open(p, "w").write(s)
print("policy patched")
