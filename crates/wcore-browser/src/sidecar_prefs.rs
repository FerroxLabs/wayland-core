//! gh#1117 — the loopback half of sidecar containment.
//!
//! [`crate::egress_proxy::PolicyEgressProxy`] puts Core in the middle of every
//! connection the browser opens *that the browser agrees to send there*.
//! Firefox does not agree for loopback: with
//! `network.proxy.allow_hijacking_localhost` at its default `false` it dials
//! `127.0.0.1` / `localhost` **itself**, around any configured proxy.
//!
//! MEASURED on hetzner-dsm 2026-08-24 against real Camoufox (`camoufox-bin`
//! from `@askjo/camofox-browser@1.13.1`'s install, headless, with a loopback
//! proxy configured through Playwright exactly the way the sidecar configures
//! it): with the pref at its default, `http://127.0.0.1:PORT/` and
//! `http://localhost:PORT/` reached the local server directly and the proxy
//! saw nothing. With the pref set to `true`, both reached the proxy and
//! nothing reached the local server except through it.
//!
//! ## Why the pref is set here and not by the sidecar
//!
//! `@askjo/camofox-browser` builds its Playwright launch options in
//! `server.js` and passes no `firefox_user_prefs`; its env allowlist
//! (`lib/config.js` `serverEnv`) forwards nothing that becomes a browser pref,
//! and Playwright replaces the browser process environment wholesale with
//! `camoufox-js`'s own. There is no seam through the sidecar.
//!
//! Firefox itself has one that does not need the sidecar's cooperation: every
//! `.js` file in the browser install's default-preference directory is read at
//! startup, before any profile. That directory is the one Camoufox ships
//! `channel-prefs.js` in, and it is how this module finds it — by the marker
//! file, not by a guessed layout, so a layout this code did not anticipate
//! fails loudly instead of writing the pref somewhere Firefox never reads.
//!
//! `pref()` sets the DEFAULT branch, so a profile's own `user.js` still wins,
//! and the file is a no-op for a browser launched without a proxy.

use std::path::{Path, PathBuf};

/// The pref that decides whether Firefox honours a configured proxy for
/// loopback destinations.
pub const LOOPBACK_PROXY_PREF: &str = "network.proxy.allow_hijacking_localhost";

/// The file Core writes. Named for Core so an operator reading their Camoufox
/// install can tell who put it there and delete it if they want the old
/// behaviour back (Core then refuses the sidecar rather than pretending).
pub const PREF_FILE_NAME: &str = "wayland-core-egress-gate.js";

/// Marker Firefox itself installs in the directory we need to write to.
/// Finding it is the whole location strategy — see the module header.
const MARKER: &str = "channel-prefs.js";

/// Body of the pref file. Deterministic, so the write is idempotent.
pub fn pref_file_contents() -> String {
    format!(
        "// Written by Wayland Core (gh#1117). Safe to delete; Core will refuse\n\
         // to run a Camoufox sidecar without it rather than run one whose\n\
         // loopback traffic it cannot screen.\n\
         //\n\
         // Firefox bypasses a configured proxy for loopback destinations\n\
         // unless this is true, which would let a loaded page reach any\n\
         // service on this machine without passing Core's egress gate.\n\
         // This is a DEFAULT-branch pref: a profile's own user.js still wins,\n\
         // and it does nothing at all for a browser launched with no proxy.\n\
         pref(\"{LOOPBACK_PROXY_PREF}\", true);\n"
    )
}

/// The operator's explicit Camoufox executable, if any. Same three names, in
/// the same order, that `@askjo/camofox-browser`'s `lib/config.js` reads —
/// whatever it launches is what Core has to reach.
fn configured_camoufox_executable() -> Option<PathBuf> {
    for key in [
        "CAMOUFOX_EXECUTABLE",
        "CAMOUFOX_EXECUTABLE_PATH",
        "CAMOFOX_EXECUTABLE_PATH",
    ] {
        if let Ok(raw) = std::env::var(key) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return Some(PathBuf::from(trimmed));
            }
        }
    }
    None
}

/// Where `camoufox-js` unpacks the browser when nobody overrode it. Mirrors
/// `camoufoxCacheDir()` in the sidecar's `lib/config.js`; the three arms are
/// that function's three arms.
fn camoufox_cache_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    if cfg!(target_os = "macos") {
        return Some(home.join("Library").join("Caches").join("camoufox"));
    }
    if cfg!(windows) {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData").join("Local"));
        return Some(base.join("camoufox").join("camoufox").join("Cache"));
    }
    let cache_root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"));
    Some(cache_root.join("camoufox"))
}

/// Every root the Camoufox install could be at, given how the sidecar
/// resolves the browser. Order is preference order; each is only a CANDIDATE
/// until [`default_pref_dir`] finds the marker under it.
///
/// An explicit executable override REPLACES the default roots rather than
/// being tried before them. Falling back would write the pref into a
/// different install than the one the sidecar launches, and then report
/// success — containment Core does not have, which is the failure mode this
/// module exists to end.
fn install_roots() -> Vec<PathBuf> {
    if let Some(exe) = configured_camoufox_executable() {
        let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
        let Some(dir) = exe.parent() else {
            return Vec::new();
        };
        let mut roots = vec![dir.to_path_buf()];
        // macOS bundle: `.../Contents/MacOS/camoufox` keeps its preferences
        // in `.../Contents/Resources`.
        if let Some(contents) = dir.parent() {
            roots.push(contents.join("Resources"));
        }
        return roots;
    }
    let Some(cache) = camoufox_cache_dir() else {
        return Vec::new();
    };
    vec![
        cache
            .join("Camoufox.app")
            .join("Contents")
            .join("Resources"),
        cache,
    ]
}

/// The directory Firefox reads default prefs from, found by its own marker
/// file rather than by assuming a layout.
pub fn default_pref_dir() -> Result<PathBuf, String> {
    let roots = install_roots();
    for root in &roots {
        let candidate = root.join("defaults").join("pref");
        if candidate.join(MARKER).is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "could not find the Camoufox install's default-preference directory \
         (no defaults/pref/{MARKER} under any of {}). Core needs it to stop the \
         browser bypassing its egress proxy for loopback destinations",
        roots
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Write the pref into `pref_dir`, and return the path written.
///
/// Idempotent: an existing file with the same body is left alone, so this
/// costs one read on every launch after the first and never rewrites a file
/// the browser may be reading.
pub fn write_loopback_pref(pref_dir: &Path) -> Result<PathBuf, String> {
    let path = pref_dir.join(PREF_FILE_NAME);
    let body = pref_file_contents();
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == body) {
        return Ok(path);
    }
    std::fs::write(&path, &body)
        .map(|()| path.clone())
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// Locate the install and put the pref in it. This is the whole production
/// entry point; `Err` means Core cannot contain the browser's loopback egress
/// and the caller must refuse rather than proceed.
pub fn contain_sidecar_loopback() -> Result<PathBuf, String> {
    write_loopback_pref(&default_pref_dir()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pref file has to be syntactically what Firefox reads out of
    /// `defaults/pref`: a `pref()` call on the DEFAULT branch. `user_pref` is
    /// a profile form and is IGNORED there, which would make the whole
    /// mechanism a silent no-op that still reports success.
    #[test]
    fn the_pref_file_sets_the_default_branch_not_a_user_pref() {
        let body = pref_file_contents();
        assert!(
            body.contains(&format!("pref(\"{LOOPBACK_PROXY_PREF}\", true);")),
            "{body}"
        );
        assert!(
            !body.contains("user_pref("),
            "user_pref is ignored in defaults/pref, so the file would do nothing: {body}"
        );
    }

    /// Build a directory that looks exactly like a Camoufox install, with or
    /// without the marker file Firefox itself puts there.
    fn fake_install(root: &std::path::Path, with_marker: bool) -> PathBuf {
        let pref_dir = root.join("defaults").join("pref");
        std::fs::create_dir_all(&pref_dir).unwrap();
        if with_marker {
            std::fs::write(pref_dir.join(MARKER), "// stand-in\n").unwrap();
        }
        let exe = root.join("camoufox-bin");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        exe
    }

    /// The locator must find the directory through the operator's explicit
    /// executable override, because that is the install the sidecar will
    /// actually launch — writing into the default cache path instead would
    /// put the pref somewhere Firefox never reads.
    #[test]
    #[serial_test::serial(camoufox_executable_env)]
    fn the_explicit_executable_override_decides_which_install_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = fake_install(tmp.path(), true);
        let _guard = EnvGuard::set("CAMOUFOX_EXECUTABLE_PATH", exe.to_str().unwrap());

        let found = default_pref_dir().expect("an install with the marker must be found");

        // Compared in the spelling the PRODUCT produces, not in `TempDir`'s.
        // `install_roots` canonicalizes the operator's executable so that
        // `.parent()` is the real install directory rather than whatever alias
        // the operator typed — and on two of the three platforms that is a
        // different STRING for the same directory: macOS resolves `/var` to
        // `/private/var`, Windows resolves to the verbatim `\\?\D:\...` form.
        // Asserting on one platform's rendering failed on Windows 11 26200 for
        // a locator that had found exactly the right directory, and would fail
        // on macOS for the same reason. Both sides are now produced the same
        // way instead of one being converted to agree with the other.
        assert_eq!(
            std::fs::canonicalize(&found).unwrap(),
            std::fs::canonicalize(tmp.path().join("defaults").join("pref")).unwrap(),
            "the override must decide the install, however this platform spells it"
        );

        // ...and the spelling the locator hands back has to be one the WRITE
        // accepts, end to end. A located directory Core cannot actually write
        // into is gh#1117's loopback hole with an `Ok` in front of it, so the
        // assertion is not that the string looks right: the pref is written
        // THROUGH the located path and then read back through the RAW tempdir
        // path, which is the one Firefox's own install sits at.
        let written = write_loopback_pref(&found).expect("the located directory must be writable");
        assert_eq!(
            std::fs::read_to_string(
                tmp.path()
                    .join("defaults")
                    .join("pref")
                    .join(PREF_FILE_NAME)
            )
            .expect("the pref must land where Firefox reads it"),
            pref_file_contents(),
            "written through {}",
            written.display()
        );
    }

    /// KNOWN-POSITIVE CONTROL for the test above, and the point of locating
    /// by marker: a directory with the right SHAPE but without the file
    /// Firefox installs is not accepted. Reporting success there would write
    /// the pref where nothing reads it, which is indistinguishable from
    /// working until someone reaches a local service.
    #[test]
    #[serial_test::serial(camoufox_executable_env)]
    fn an_install_without_the_firefox_marker_is_refused_not_guessed_at() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = fake_install(tmp.path(), false);
        let _guard = EnvGuard::set("CAMOUFOX_EXECUTABLE_PATH", exe.to_str().unwrap());

        let error = default_pref_dir().expect_err("a shape-only match must not be accepted");
        assert!(error.contains(MARKER), "{error}");
    }

    #[test]
    fn writing_the_pref_leaves_an_up_to_date_file_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let first = write_loopback_pref(tmp.path()).unwrap();
        let mtime_before = std::fs::metadata(&first).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = write_loopback_pref(tmp.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            mtime_before,
            std::fs::metadata(&second).unwrap().modified().unwrap(),
            "the second write rewrote a file the browser may be reading"
        );
        assert_eq!(
            std::fs::read_to_string(&second).unwrap(),
            pref_file_contents()
        );
    }

    /// A file left by an older Core must be corrected. Skipping the rewrite
    /// on "a file is already there" would make every upgrade a no-op on every
    /// machine that ran the previous version.
    #[test]
    fn a_stale_pref_file_from_an_older_core_is_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(PREF_FILE_NAME);
        std::fs::write(&path, "pref(\"something.else\", false);\n").unwrap();
        write_loopback_pref(tmp.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            pref_file_contents()
        );
    }

    /// An install Core cannot write to must be an error. Returning `Ok` there
    /// reports containment Core does not have, which is the exact failure
    /// mode gh#1117 exists to end.
    ///
    /// Uses a path with a FILE as a directory component rather than a
    /// permission bit, so it still bites when the tests run as root — where a
    /// mode-0500 directory is writable and the assertion could never fail.
    #[test]
    fn an_unwritable_install_is_an_error_not_a_shrug() {
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("not-a-directory");
        std::fs::write(&blocker, "").unwrap();

        let error = write_loopback_pref(&blocker.join("defaults").join("pref"))
            .expect_err("a path that cannot hold a file reported success");
        assert!(error.contains(PREF_FILE_NAME), "{error}");
        // KNOWN-POSITIVE CONTROL: the same call against a real directory
        // succeeds, so the error above is the path and not the function.
        assert!(write_loopback_pref(tmp.path()).is_ok());
    }

    /// Restores the previous value of one env var on drop, so a failing test
    /// cannot leak `CAMOUFOX_EXECUTABLE_PATH` into the rest of the binary.
    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: the two tests that touch this key are `serial` on the
            // same key, so no other thread in this binary is reading or
            // writing the environment concurrently.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: as above.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
