//! F25-04 — the twelve-verb plugin lifecycle, driven through the REAL binary.
//!
//! Every assertion here spawns `wayland-core plugin <verb>` and reads its exit
//! code and stdout. Calling the underlying library function would prove the
//! function; Success Criterion 3 is a list of OPERATOR ACTIONS, and only the
//! shipped binary can prove one of those.
//!
//! And the printed line is never the evidence — after each verb this file
//! observes the STATE CHANGE on disk independently. A verb implemented as a
//! stub would print a plausible success and change nothing, which is exactly
//! the failure these tests exist to catch.

#![allow(clippy::panic, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_wayland-core");

struct Run {
    output: Output,
    argv: String,
}

impl Run {
    fn code(&self) -> i32 {
        self.output.status.code().unwrap_or(-1)
    }
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).to_string()
    }
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).to_string()
    }
    fn ok(self) -> Self {
        assert_eq!(
            self.code(),
            0,
            "expected success from `{}`\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.argv,
            self.stdout(),
            self.stderr()
        );
        self
    }
    fn failed(self) -> Self {
        assert_ne!(
            self.code(),
            0,
            "expected a NON-ZERO exit from `{}` — a verb that cannot fail is not a gate\
             \n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.argv,
            self.stdout(),
            self.stderr()
        );
        self
    }
    fn stdout_contains(self, needle: &str) -> Self {
        assert!(
            self.stdout().contains(needle),
            "`{}` stdout did not contain {needle:?}\n--- stdout ---\n{}",
            self.argv,
            self.stdout()
        );
        self
    }
    fn any_contains(self, needle: &str) -> Self {
        let all = format!("{}{}", self.stdout(), self.stderr());
        assert!(
            all.contains(needle),
            "`{}` output did not contain {needle:?}\n--- combined ---\n{all}",
            self.argv
        );
        self
    }
}

/// Spawn the shipped binary with an isolated profile so nothing touches a real
/// user's plugin store.
fn plugin(root: &Path, args: &[&str]) -> Run {
    let mut cmd = Command::new(BIN);
    cmd.arg("plugin")
        .args(args)
        .arg("--install-root")
        .arg(root)
        // Keep every profile-derived path inside the sandbox.
        .env("WAYLAND_HOME", root.parent().unwrap_or(root))
        .env("WAYLAND_PLUGINS_DIR", root);
    let argv = format!(
        "wayland-core plugin {} --install-root {}",
        args.join(" "),
        root.display()
    );
    let output = cmd.output().expect("spawn wayland-core");
    Run { output, argv }
}

/// A minimal but REAL plugin source tree: a declarative manifest plus an entry
/// artifact so the whole lifecycle — including signing, which needs bytes to
/// sign — applies to one plugin rather than being split across two.
fn author_plugin(dir: &Path, version: &str, payload: &[u8]) {
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::write(dir.join("bin").join("run"), payload).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        format!(
            "plugin_api_version = \"{api}\"\n\
             [plugin]\n\
             name = \"lifecycle-demo\"\n\
             version = \"{version}\"\n\
             description = \"F25-04 lifecycle fixture\"\n\
             license = \"MIT\"\n\
             [permissions]\n\
             register_hooks = true\n\
             [runtime]\n\
             kind = \"subprocess\"\n\
             [runtime.subprocess]\n\
             binary_path = \"bin/run\"\n",
            api = wcore_plugin_api::PLUGIN_API_VERSION
        ),
    )
    .unwrap();
}

fn installed_dir(root: &Path) -> PathBuf {
    root.join("lifecycle-demo@market")
}

fn digest(dir: &Path) -> String {
    wcore_config::plugin_governance::content_digest(dir).unwrap()
}

fn approvals(root: &Path) -> wcore_config::plugin_governance::ApprovalStore {
    wcore_config::plugin_governance::load_approvals(root).unwrap()
}

/// Set up an author dir, key, market dir and install root under one tempdir.
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    src: PathBuf,
    market: PathBuf,
    key: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("home");
    let root = base.join("plugins");
    let src = tmp.path().join("src").join("lifecycle-demo");
    let market = tmp.path().join("market");
    std::fs::create_dir_all(&root).unwrap();
    author_plugin(&src, "1.0.0", b"payload v1");
    let key = tmp.path().join("author.key");
    plugin(&root, &["sign", "--new-key", key.to_str().unwrap()]).ok();
    assert!(key.is_file(), "sign --new-key did not write the key");
    assert!(
        key.with_extension("pub").is_file(),
        "sign --new-key did not write the verifying key"
    );
    Fixture {
        _tmp: tmp,
        root,
        src,
        market,
        key,
    }
}

/// Publish the current `src` into the market and install it, returning the
/// installed digest.
fn publish_and_install(f: &Fixture) -> String {
    plugin(
        &f.root,
        &[
            "sign",
            f.src.to_str().unwrap(),
            "--key",
            f.key.to_str().unwrap(),
        ],
    )
    .ok();
    plugin(
        &f.root,
        &[
            "publish",
            f.src.to_str().unwrap(),
            "--to",
            f.market.to_str().unwrap(),
        ],
    )
    .ok();
    plugin(&f.root, &["marketplace", "add", f.market.to_str().unwrap()]).ok();
    plugin(&f.root, &["install", "lifecycle-demo@market"]).ok();
    digest(&installed_dir(&f.root))
}

// ---------------------------------------------------------------------------
// Author-side verbs
// ---------------------------------------------------------------------------

/// `plugin verify` must be able to go RED. An API version the host does not
/// implement is a non-zero exit, not a printed warning.
#[test]
fn verify_is_a_gate_that_can_fail() {
    let f = fixture();

    plugin(&f.root, &["verify", f.src.to_str().unwrap()])
        .ok()
        .stdout_contains("COMPATIBLE")
        .stdout_contains("VERIFIED");

    let toml = std::fs::read_to_string(f.src.join("plugin.toml")).unwrap();
    std::fs::write(
        f.src.join("plugin.toml"),
        toml.replace(
            wcore_plugin_api::PLUGIN_API_VERSION,
            "999.0-from-the-future",
        ),
    )
    .unwrap();

    plugin(&f.root, &["verify", f.src.to_str().unwrap()])
        .failed()
        .stdout_contains("INCOMPATIBLE");
}

/// `plugin sign` produces a signature the ENGINE's verifier accepts, and one
/// mutated byte is rejected by that same verifier. Not a bespoke check — the
/// real `sig_verifier` entry point.
#[test]
fn sign_produces_a_signature_the_engine_verifier_accepts_and_a_mutation_breaks_it() {
    let f = fixture();
    plugin(
        &f.root,
        &[
            "sign",
            f.src.to_str().unwrap(),
            "--key",
            f.key.to_str().unwrap(),
        ],
    )
    .ok()
    .stdout_contains("wayland-plugin.sig");

    let sig = f.src.join("bin").join("wayland-plugin.sig");
    assert!(sig.is_file(), "no signature on disk after `plugin sign`");
    assert_eq!(
        std::fs::metadata(&sig).unwrap().len(),
        64,
        "detached ed25519 signatures are exactly 64 bytes"
    );

    let keys_dir = f.src.parent().unwrap().join("trusted");
    std::fs::create_dir_all(&keys_dir).unwrap();
    std::fs::copy(f.key.with_extension("pub"), keys_dir.join("author.pub")).unwrap();
    let union = wcore_agent::plugins::sig_verifier::load_filesystem_keys(&keys_dir);
    assert_eq!(union.len(), 1, "trust anchor did not load the key");

    let entry = f.src.join("bin").join("run");
    wcore_agent::plugins::sig_verifier::verify_path_plugin_signature(
        "lifecycle-demo",
        &entry,
        &union,
    )
    .expect("the engine's own verifier must accept what `plugin sign` produced");

    // One byte.
    std::fs::write(&entry, b"payload v2").unwrap();
    assert!(
        wcore_agent::plugins::sig_verifier::verify_path_plugin_signature(
            "lifecycle-demo",
            &entry,
            &union,
        )
        .is_err(),
        "a mutated entry artifact must NOT verify"
    );
}

/// Publishing unsigned material is refused, and a published bundle is
/// digest-addressed.
#[test]
fn publish_refuses_unsigned_material_and_records_a_digest() {
    let f = fixture();
    plugin(
        &f.root,
        &[
            "publish",
            f.src.to_str().unwrap(),
            "--to",
            f.market.to_str().unwrap(),
        ],
    )
    .failed()
    .any_contains("unsigned");
    assert!(
        !f.market.join("plugins").join("lifecycle-demo").exists(),
        "a refused publish still wrote to the target"
    );

    plugin(
        &f.root,
        &[
            "sign",
            f.src.to_str().unwrap(),
            "--key",
            f.key.to_str().unwrap(),
        ],
    )
    .ok();
    plugin(
        &f.root,
        &[
            "publish",
            f.src.to_str().unwrap(),
            "--to",
            f.market.to_str().unwrap(),
        ],
    )
    .ok()
    .stdout_contains("published lifecycle-demo");

    let published = f.market.join("plugins").join("lifecycle-demo");
    assert!(published.join("bundle.json").is_file());
    assert!(
        published.join("bin").join("wayland-plugin.sig").is_file(),
        "the published tree must carry the signature beside its entry artifact"
    );
    assert!(
        f.market
            .join(".claude-plugin")
            .join("marketplace.json")
            .is_file(),
        "publish did not produce a catalog the existing marketplace path can read"
    );
}

/// `plugin new` either scaffolds or refuses with the install command. It never
/// prints success over an empty directory.
#[test]
fn new_scaffolds_or_refuses_but_never_half_succeeds() {
    let f = fixture();
    let dest = f.src.parent().unwrap().join("scaffolded");
    let run = plugin(
        &f.root,
        &["new", "smoke-plugin", "--path", dest.to_str().unwrap()],
    );
    if run.code() == 0 {
        let out = dest.join("smoke-plugin");
        assert!(
            out.join("Cargo.toml").is_file(),
            "scaffold reported success but produced no Cargo.toml"
        );
    } else {
        let all = format!("{}{}", run.stdout(), run.stderr());
        assert!(
            all.contains("cargo install cargo-generate"),
            "a refusal must name the exact install command; got:\n{all}"
        );
        assert!(
            !dest.join("smoke-plugin").exists(),
            "a refused scaffold still created the output directory"
        );
    }
}

/// An invalid plugin name never reaches `cargo generate`.
#[test]
fn new_rejects_a_traversing_plugin_name() {
    let f = fixture();
    plugin(
        &f.root,
        &["new", "../escape", "--path", f.market.to_str().unwrap()],
    )
    .failed();
}

// ---------------------------------------------------------------------------
// Operator-side verbs
// ---------------------------------------------------------------------------

/// NEGATIVE CASE 1, and the most important test in this file: an installed but
/// unapproved plugin is REFUSED, approving admits it, revoking re-arms the
/// refusal. The verdict asserted here is the loader's own — the same
/// `plugin_governance::evaluate` the engine calls — not a CLI opinion.
#[test]
fn an_unapproved_plugin_is_refused_and_approval_is_reversible() {
    use wcore_config::plugin_governance::{GateVerdict, evaluate};
    let f = fixture();
    let installed_digest = publish_and_install(&f);
    let dir = installed_dir(&f.root);

    // Installed, and the CLI says so plainly.
    plugin(&f.root, &["inspect", "lifecycle-demo"])
        .ok()
        .stdout_contains("NO — the loader will refuse this plugin")
        .stdout_contains(&installed_digest);
    assert!(
        matches!(
            evaluate(&f.root, "lifecycle-demo", &dir),
            GateVerdict::Refused { .. }
        ),
        "the gate admitted a plugin that was never approved"
    );
    assert!(approvals(&f.root).approvals.is_empty());

    // Approve → admitted.
    plugin(&f.root, &["approve", "lifecycle-demo"])
        .ok()
        .stdout_contains("approved lifecycle-demo");
    assert!(matches!(
        evaluate(&f.root, "lifecycle-demo", &dir),
        GateVerdict::Approved { .. }
    ));
    assert_eq!(
        approvals(&f.root).approvals["lifecycle-demo"].digest,
        installed_digest,
        "the approval must be bound to the installed bytes"
    );
    plugin(&f.root, &["inspect", "lifecycle-demo"])
        .ok()
        .stdout_contains("the approval gate admits this plugin");

    // Revoke → refused again.
    plugin(&f.root, &["approve", "lifecycle-demo", "--revoke"])
        .ok()
        .stdout_contains("REFUSED at load until re-approved");
    assert!(matches!(
        evaluate(&f.root, "lifecycle-demo", &dir),
        GateVerdict::Refused { .. }
    ));
    assert!(
        approvals(&f.root)
            .revoked
            .iter()
            .any(|r| r.digest == installed_digest),
        "the revocation must be retained so recovery cannot undo it"
    );
}

/// NEGATIVE CASE 2: a bundle whose bytes changed after publication refuses to
/// install. The refusal is a non-zero exit, and nothing lands in the store.
#[test]
fn a_tampered_bundle_refuses_to_install() {
    let f = fixture();
    plugin(
        &f.root,
        &[
            "sign",
            f.src.to_str().unwrap(),
            "--key",
            f.key.to_str().unwrap(),
        ],
    )
    .ok();
    plugin(
        &f.root,
        &[
            "publish",
            f.src.to_str().unwrap(),
            "--to",
            f.market.to_str().unwrap(),
        ],
    )
    .ok();
    plugin(&f.root, &["marketplace", "add", f.market.to_str().unwrap()]).ok();

    // Tamper: one byte inside the published tree.
    let entry = f
        .market
        .join("plugins")
        .join("lifecycle-demo")
        .join("bin")
        .join("run");
    let mut bytes = std::fs::read(&entry).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&entry, &bytes).unwrap();

    plugin(&f.root, &["install", "lifecycle-demo@market"])
        .failed()
        .any_contains("integrity check FAILED");
}

/// `update` retains the prior generation; `rollback` restores it byte for byte.
/// NEGATIVE CASE 3 — proven by digest equality with the pre-update digest, not
/// by the verb's own claim.
#[test]
fn update_retains_and_rollback_restores_byte_identical_prior_content() {
    let f = fixture();
    let v1_digest = publish_and_install(&f);
    let dir = installed_dir(&f.root);
    let v1_bytes = std::fs::read(dir.join("bin").join("run")).unwrap();

    plugin(&f.root, &["approve", "lifecycle-demo"]).ok();

    // Publish a second version and update into it.
    author_plugin(&f.src, "2.0.0", b"payload v2 -- different bytes");
    plugin(
        &f.root,
        &[
            "sign",
            f.src.to_str().unwrap(),
            "--key",
            f.key.to_str().unwrap(),
        ],
    )
    .ok();
    plugin(
        &f.root,
        &[
            "publish",
            f.src.to_str().unwrap(),
            "--to",
            f.market.to_str().unwrap(),
        ],
    )
    .ok();

    plugin(&f.root, &["update", "lifecycle-demo"])
        .ok()
        .stdout_contains("updated lifecycle-demo");
    let v2_digest = digest(&dir);
    assert_ne!(v1_digest, v2_digest, "update did not change the live bytes");

    // The prior generation is retained on disk, not merely claimed.
    plugin(&f.root, &["inspect", "lifecycle-demo"])
        .ok()
        .stdout_contains("retained      2 generation(s)");

    // The old approval did NOT survive the change of bytes.
    assert!(
        matches!(
            wcore_config::plugin_governance::evaluate(&f.root, "lifecycle-demo", &dir),
            wcore_config::plugin_governance::GateVerdict::Refused { .. }
        ),
        "an update must invalidate the prior approval, not inherit it"
    );

    plugin(&f.root, &["rollback", "lifecycle-demo"])
        .ok()
        .stdout_contains("rolled back lifecycle-demo");

    assert_eq!(
        digest(&dir),
        v1_digest,
        "rollback did not restore the pre-update digest"
    );
    assert_eq!(
        std::fs::read(dir.join("bin").join("run")).unwrap(),
        v1_bytes,
        "rollback restored something other than the exact prior bytes"
    );
}

/// NEGATIVE CASE 4: `recover` repairs damage INDUCED on disk — not simulated
/// with a flag, because a flag proves the flag.
#[test]
fn recover_repairs_induced_half_written_state() {
    let f = fixture();
    let v1_digest = publish_and_install(&f);
    let dir = installed_dir(&f.root);
    plugin(&f.root, &["approve", "lifecycle-demo"]).ok();

    // A sound store must recover to "nothing to do", or the verb manufactures
    // repairs and its report proves nothing.
    plugin(&f.root, &["recover"])
        .ok()
        .stdout_contains("nothing to repair");

    // Damage 1: delete the live install directory out from under the ledger.
    std::fs::remove_dir_all(&dir).unwrap();
    // Damage 2: leave a staging directory behind, as an interrupted write does.
    let staging = f
        .root
        .join(".generations")
        .join("lifecycle-demo")
        .join(".staging-abc123");
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("half"), b"..").unwrap();

    plugin(&f.root, &["recover"])
        .ok()
        .stdout_contains("repaired");

    assert!(
        dir.is_dir(),
        "recover did not restore the install directory"
    );
    assert_eq!(digest(&dir), v1_digest, "recover restored the wrong bytes");
    assert!(
        !staging.exists(),
        "recover left the interrupted staging dir"
    );
    // Recovery restores BYTES. The approval was untouched, so it still holds.
    assert_eq!(
        approvals(&f.root).approvals["lifecycle-demo"].digest,
        v1_digest
    );
}

/// Recovery must never hand back an authority a human withdrew.
#[test]
fn recover_does_not_resurrect_a_revoked_approval() {
    let f = fixture();
    let v1_digest = publish_and_install(&f);
    let dir = installed_dir(&f.root);
    plugin(&f.root, &["approve", "lifecycle-demo"]).ok();
    plugin(&f.root, &["approve", "lifecycle-demo", "--revoke"]).ok();

    std::fs::remove_dir_all(&dir).unwrap();
    plugin(&f.root, &["recover"]).ok();

    assert_eq!(digest(&dir), v1_digest, "bytes should have come back");
    assert!(
        matches!(
            wcore_config::plugin_governance::evaluate(&f.root, "lifecycle-demo", &dir),
            wcore_config::plugin_governance::GateVerdict::Refused { .. }
        ),
        "recovery restored an authority that was revoked"
    );
}

/// `inspect` of an unknown plugin exits non-zero so it is scriptable.
#[test]
fn inspect_of_an_absent_plugin_exits_non_zero() {
    let f = fixture();
    plugin(&f.root, &["inspect", "no-such-plugin"]).failed();
}

/// `remove` takes the plugin off disk and takes its governance state with it.
#[test]
fn remove_clears_the_install_and_its_lifecycle_state() {
    let f = fixture();
    publish_and_install(&f);
    plugin(&f.root, &["approve", "lifecycle-demo"]).ok();
    let dir = installed_dir(&f.root);
    assert!(dir.is_dir());

    plugin(&f.root, &["remove", "lifecycle-demo"])
        .ok()
        .stdout_contains("removed lifecycle-demo");

    assert!(!dir.exists(), "remove left the install directory on disk");
    assert!(
        approvals(&f.root).approvals.is_empty(),
        "remove left a stale approval that would pre-approve a reinstall"
    );
    plugin(&f.root, &["inspect", "lifecycle-demo"]).failed();
}

/// All twelve verbs are reachable from the shipped binary's help.
#[test]
fn all_twelve_lifecycle_verbs_appear_in_plugin_help() {
    let out = Command::new(BIN)
        .args(["plugin", "--help"])
        .output()
        .expect("spawn wayland-core plugin --help");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    for verb in [
        "new", "test", "verify", "sign", "publish", "install", "approve", "inspect", "update",
        "rollback", "recover", "remove",
    ] {
        assert!(
            text.lines()
                .any(|l| l.split_whitespace().next() == Some(verb)),
            "verb `{verb}` is absent from `plugin --help`:\n{text}"
        );
    }
}

/// A plugins root that predates the lifecycle keeps loading exactly as before.
/// This is the migration promise of root-scoped governance, and it has to be
/// testable or it is only an intention.
#[test]
fn an_ungoverned_plugins_root_is_not_gated() {
    use wcore_config::plugin_governance::{GateVerdict, evaluate};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("plugins");
    let dir = root.join("legacy");
    author_plugin(&dir, "1.0.0", b"legacy");
    assert_eq!(
        evaluate(&root, "lifecycle-demo", &dir),
        GateVerdict::NotGoverned
    );
}
