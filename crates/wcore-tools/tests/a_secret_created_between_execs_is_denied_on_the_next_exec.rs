//! FerroxLabs/wayland#1111 bullet 1, graded from the direction the refutation
//! actually rests on.
//!
//! `secret_walk_call_count_test.rs` refutes the memoisation the bullet asks for
//! by showing the correct deny list changing when a grant EXPIRES. That is the
//! shrinking direction. The growing direction — a secret that did not exist
//! when the previous exec was set up — is the one the refutation argues in
//! prose (`secret_deny_paths_dynamic`'s "a secret CREATED AFTER bootstrap … is
//! denied on the very next Bash command") and the one that leaks if it is
//! wrong: a cached list computed before the file existed leaves the sandboxed
//! child free to read it.
//!
//! Nothing here depends on the clock, on a grant, or on any mutating call into
//! the policy. The ONLY way the second read can see the new file is by walking
//! the workspace again, so this is a red arm for any cache added to
//! `secret_deny_paths_dynamic`.

use std::path::PathBuf;
use wcore_tools::workspace_policy::{WorkspacePolicy, walk_calls};

fn denied(p: &WorkspacePolicy) -> Vec<PathBuf> {
    p.secret_deny_paths_for_backend(true)
}

#[test]
fn a_secret_created_between_execs_is_denied_on_the_next_exec() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
    std::fs::write(root.join(".env"), b"A=1").unwrap();
    let policy = WorkspacePolicy::contained(&root);

    let before = walk_calls();
    let first = denied(&policy);
    // POSITIVE CONTROL: the pre-existing secret is found, so a miss below is
    // a miss and not a broken predicate.
    assert!(
        first.iter().any(|p| p.ends_with(".env")),
        "control: pre-existing .env missing from {first:?}"
    );
    assert!(
        !first.iter().any(|p| p.ends_with("id_rsa")),
        "id_rsa exists before it was created: {first:?}"
    );

    // The child of exec #1 writes a new secret. No VFS call, no grant change,
    // no clock dependence - the ONLY way to see it is to walk again.
    std::fs::create_dir_all(root.join("deploy")).unwrap();
    std::fs::write(root.join("deploy/id_rsa"), b"-----BEGIN-----").unwrap();

    let second = denied(&policy);
    let walks = walk_calls() - before;
    assert!(
        second.iter().any(|p| p.ends_with("id_rsa")),
        "a secret created by the previous exec is NOT denied on the next one \
         ({second:?}) - the sandboxed child can read it. walks={walks}"
    );
    assert_eq!(
        walks, 2,
        "two execs walked {walks} times - a cache has been added, and the \
         assertion above would then be reading a list computed before the \
         secret existed"
    );
}
