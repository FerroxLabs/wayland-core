
    /// core#322 c4 — THE TIEBREAK. The lane graded c4 met on a parity claim: the
    /// walk asks [`wcore_tools::workspace_policy::is_vcs_store_or_control_dir`],
    /// which tests the path ITSELF, and the deny walk asks `inside_vcs_store`,
    /// which tests the path and every ANCESTOR. The lane's defence was that a
    /// walk PRUNES at the control directory and therefore can never stand
    /// inside a store, making the two equivalent in effect.
    ///
    /// Pruning only governs paths the walk DESCENDS to. A symlink is an entry
    /// the walk meets at the top of the tree, and one aimed BELOW a store's own
    /// root — `.git/objects/aa`, not `.git` and not `.git/objects` — resolves to
    /// a path that is neither a store shape (`objects/aa` is not a
    /// (control, store) pair) nor a control-directory leaf. The self-test says
    /// walk it; the ancestor test says deny it; the difference is a committed
    /// object in the payload.
    #[cfg(unix)]
    #[test]
    fn at_dir_prunes_a_path_that_resolves_below_a_store_root() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::create_dir_all(root.join(".git/objects/aa")).expect("mkdir git store");
        fs::write(root.join(".git/objects/aa/deadbeef"), COMMITTED_OBJECT).expect("write object");

        // Aimed below the store root — the input on which the two predicates
        // disagree.
        std::os::unix::fs::symlink(root.join(".git/objects/aa"), root.join("shortcut"))
            .expect("symlink dir");
        // The same reach on the FILE arm, which never consulted a store
        // predicate at all: `is_secret_path` matches secret NAMES, and an
        // object file is named after its hash.
        std::os::unix::fs::symlink(
            root.join(".git/objects/aa/deadbeef"),
            root.join("blob.txt"),
        )
        .expect("symlink file");

        // Wrong-refusal controls, so the fix cannot be "prune every link".
        fs::write(root.join("ok.txt"), "safe\n").expect("write ok");
        std::os::unix::fs::symlink(root.join("ok.txt"), root.join("alias.txt"))
            .expect("symlink ok");

        let payload = resolve(&AtRef::parse("@./").expect("parse"), root).expect("resolve dir");
        let leaked: Vec<String> = payload
            .files
            .iter()
            .filter(|f| f.content.contains("COMMITTED-OBJECT"))
            .map(|f| f.path.display().to_string())
            .collect();
        assert!(
            leaked.is_empty(),
            "a path resolving BELOW a VCS store root was attached — the walk's \
             self-test is not the deny walk's ancestor test: {leaked:?}"
        );
        assert_eq!(
            payload
                .files
                .iter()
                .filter(|f| f.content == "safe\n")
                .count(),
            2,
            "control: an ordinary file and an ordinary link to it must both stay attached"
        );
    }
}
