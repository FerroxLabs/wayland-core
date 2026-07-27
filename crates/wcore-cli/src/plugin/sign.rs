// F25-04: `plugin sign` — detached Ed25519 signature over the plugin's entry
// artifact.
//
// THIS MINTS NO NEW TRUST ROOT. The engine already has exactly one:
// `wcore_agent::plugins::sig_verifier` reads `wayland-plugin.sig` (a raw
// 64-byte Ed25519 signature over the entry artifact's bytes) and verifies it
// against the `*.pub` files in the trust-anchor directory. This verb produces
// precisely that file, over precisely those bytes. Two answers to "who is
// trusted" is the defect class this phase exists to close, so a plugin whose
// manifest declares NO entry artifact is REFUSED rather than signed under some
// second scheme invented for the occasion.

use std::path::Path;

use ed25519_dalek::{SigningKey, ed25519::signature::Signer};
use wcore_agent::plugins::sig_verifier::PLUGIN_SIG_FILENAME;

use crate::plugin::error::{PluginCliError, Result};
use crate::plugin::verify;

/// Load a raw 32-byte Ed25519 signing key from `path`.
///
/// Raw bytes only — the same shape the trust anchor uses for public keys. The
/// key is read, used, and dropped; it is never copied into the plugin
/// directory, the bundle, or any output line.
pub fn load_signing_key(path: &Path) -> Result<SigningKey> {
    let bytes = std::fs::read(path)
        .map_err(|e| PluginCliError::Quarantine(format!("signing key {}: {e}", path.display())))?;
    let arr: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
        PluginCliError::Quarantine(format!(
            "signing key {} is {} bytes; expected 32 raw Ed25519 bytes \
             (generate one with `wayland-core plugin sign --new-key <path>`)",
            path.display(),
            bytes.len()
        ))
    })?;
    Ok(SigningKey::from_bytes(&arr))
}

/// Write a fresh keypair: `<path>` (32 raw private bytes, mode 0600 on Unix)
/// and `<path>.pub` (32 raw public bytes, droppable straight into the
/// trust-anchor directory).
pub fn generate_key(path: &Path) -> Result<()> {
    use rand::RngCore;
    if path.exists() {
        return Err(PluginCliError::Quarantine(format!(
            "{} already exists — refusing to overwrite a signing key",
            path.display()
        )));
    }
    let mut secret = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let sk = SigningKey::from_bytes(&secret);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, sk.to_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let pubpath = path.with_extension("pub");
    std::fs::write(&pubpath, sk.verifying_key().as_bytes())?;
    println!("wrote signing key   {}", path.display());
    println!("wrote verifying key {}", pubpath.display());
    println!(
        "install the verifying key as a trust anchor:\n  \
         cp {} \"${{WAYLAND_TRUSTED_KEYS_DIR:-$HOME/.wayland/trusted-keys}}/\"",
        pubpath.display()
    );
    Ok(())
}

/// Where the engine looks for a plugin's detached signature.
///
/// `verify_path_plugin_signature` reads `<entry_artifact.parent()>/wayland-plugin.sig`
/// — NEXT TO THE ARTIFACT, not at the plugin root. For a manifest declaring
/// `binary_path = "bin/run"` that is `bin/wayland-plugin.sig`. Writing it at the
/// plugin root instead produces a signature the verifier never finds, and the
/// plugin then fails to load with `SignatureMissing` while `plugin sign`
/// reported success. This helper exists so exactly one place decides the
/// location, and it derives it the same way the verifier does.
pub fn signature_path_for(entry: &Path) -> std::path::PathBuf {
    entry.parent().unwrap_or(entry).join(PLUGIN_SIG_FILENAME)
}

/// Sign `<dir>`'s entry artifact and write the detached signature where the
/// engine's verifier will look for it.
pub fn sign_dir(dir: &Path, key_path: &Path) -> Result<String> {
    let manifest = verify::load_manifest(dir)?;
    let Some(entry) = verify::entry_artifact(dir, &manifest)? else {
        return Err(PluginCliError::Quarantine(format!(
            "{} declares no entry artifact, so there are no bytes for the engine's \
             signature verifier to check. Signing it would require a SECOND signing \
             scheme, which this phase forbids. Declare a [runtime.subprocess] \
             binary_path or a [runtime.wasm] component_path first.",
            manifest.plugin.name
        )));
    };
    if !entry.is_file() {
        return Err(PluginCliError::Quarantine(format!(
            "declared entry artifact {} does not exist",
            entry.display()
        )));
    }
    let key = load_signing_key(key_path)?;
    let bytes = std::fs::read(&entry)?;
    let sig = key.sign(&bytes);
    std::fs::write(signature_path_for(&entry), sig.to_bytes())?;
    Ok(entry
        .strip_prefix(dir)
        .unwrap_or(&entry)
        .display()
        .to_string())
}

/// `plugin sign <dir> --key <path>` / `plugin sign --new-key <path>`.
pub fn run(dir: Option<&Path>, key: Option<&Path>, new_key: Option<&Path>) -> Result<()> {
    if let Some(p) = new_key {
        return generate_key(p);
    }
    let (Some(dir), Some(key)) = (dir, key) else {
        return Err(PluginCliError::Quarantine(
            "usage: plugin sign <dir> --key <signing-key> | plugin sign --new-key <path>".into(),
        ));
    };
    let signed = sign_dir(dir, key)?;
    println!(
        "signed {} → {}",
        signed,
        signature_path_for(&dir.join(&signed)).display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, VerifyingKey, ed25519::signature::Verifier};
    use tempfile::TempDir;

    fn subprocess_plugin(dir: &Path, body: &[u8]) {
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("run"), body).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                "plugin_api_version = \"{}\"\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n\
                 description = \"d\"\nlicense = \"MIT\"\n\
                 [permissions]\nregister_tools = true\ntool_namespace = \"Demo\"\n\
                 [runtime]\nkind = \"subprocess\"\n\
                 [runtime.subprocess]\nbinary_path = \"bin/run\"\n",
                wcore_plugin_api::PLUGIN_API_VERSION
            ),
        )
        .unwrap();
    }

    #[test]
    fn signature_verifies_and_a_single_mutated_byte_does_not() {
        let tmp = TempDir::new().unwrap();
        let plugin = tmp.path().join("p");
        subprocess_plugin(&plugin, b"entry bytes");
        let key_path = tmp.path().join("k.key");
        generate_key(&key_path).unwrap();

        sign_dir(&plugin, &key_path).unwrap();

        // The signature must land where the ENGINE looks: beside the entry
        // artifact, not at the plugin root. Writing it at the root produced a
        // `plugin sign` that reported success and a loader that then refused
        // the plugin with `SignatureMissing` — found by driving the engine's
        // own verifier rather than a local re-implementation of it.
        let entry = plugin.join("bin").join("run");
        assert!(
            signature_path_for(&entry).is_file(),
            "signature is not beside the entry artifact"
        );
        assert!(
            !plugin.join(PLUGIN_SIG_FILENAME).is_file(),
            "signature was written at the plugin root, where the verifier never looks"
        );

        let vk_bytes = std::fs::read(key_path.with_extension("pub")).unwrap();
        let vk = VerifyingKey::from_bytes(&vk_bytes.try_into().unwrap()).unwrap();
        let sig_bytes = std::fs::read(signature_path_for(&entry)).unwrap();
        let sig = Signature::from_bytes(&sig_bytes.try_into().unwrap());

        assert!(vk.verify(b"entry bytes", &sig).is_ok());
        // One byte changed: `s` → `t`.
        assert!(vk.verify(b"entry bytet", &sig).is_err());
    }

    #[test]
    fn signing_a_plugin_with_no_entry_artifact_is_refused_not_improvised() {
        let tmp = TempDir::new().unwrap();
        let plugin = tmp.path().join("p");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.toml"),
            format!(
                "plugin_api_version = \"{}\"\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n\
                 description = \"d\"\nlicense = \"MIT\"\n[permissions]\nregister_hooks = true\n\
                 [runtime]\nkind = \"declarative\"\n",
                wcore_plugin_api::PLUGIN_API_VERSION
            ),
        )
        .unwrap();
        let key_path = tmp.path().join("k.key");
        generate_key(&key_path).unwrap();
        let err = sign_dir(&plugin, &key_path).unwrap_err();
        assert!(err.to_string().contains("SECOND signing scheme"), "{err:?}");
        assert!(!plugin.join(PLUGIN_SIG_FILENAME).exists());
        let _ = PLUGIN_SIG_FILENAME;
    }

    #[test]
    fn generate_key_refuses_to_clobber_an_existing_key() {
        let tmp = TempDir::new().unwrap();
        let k = tmp.path().join("k.key");
        generate_key(&k).unwrap();
        assert!(generate_key(&k).is_err());
    }

    #[test]
    fn a_wrong_sized_key_file_is_rejected_with_its_size() {
        let tmp = TempDir::new().unwrap();
        let k = tmp.path().join("short.key");
        std::fs::write(&k, b"too short").unwrap();
        let err = load_signing_key(&k).unwrap_err();
        assert!(err.to_string().contains("9 bytes"), "{err:?}");
    }
}
