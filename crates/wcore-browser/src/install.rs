//! gh#491 — one source of truth for "which browser backend is in this build,
//! and what installs it".
//!
//! The same missing dependency used to be reported three different ways, and
//! one of them named software that is not in the shipped binary at all:
//!
//!   * the liveness probe said `@askjo/camofox-browser` / `WAYLAND_CAMOUFOX_BIN`;
//!   * the runtime refusal a user actually hit said `CAMOUFOX_EXECUTABLE_PATH`;
//!   * `--doctor` said `apt install chromium-browser` — but `chromium` is an
//!     opt-in cargo feature (`default = []`), so on the shipped artifact it
//!     recommended installing a backend that is not compiled in.
//!
//! Every one of those surfaces now derives its program name, its env override
//! and its install instruction from the constants below, so they cannot drift
//! apart again. [`compiled_backends`] is `cfg`-gated on the same features that
//! decide which backend code is linked, which is what makes "name the backend
//! that is actually compiled in" structural rather than a comment.

use std::path::PathBuf;

/// A browser backend that is compiled into this build and needs something
/// installed on the host before it can run.
#[derive(Debug, Clone, Copy)]
pub struct BackendInstall {
    /// How the backend is named to a human, e.g. "Camoufox sidecar".
    pub backend: &'static str,
    /// Programs to look for, in preference order.
    pub programs: &'static [&'static str],
    /// Env var that overrides the program, when the backend reads one.
    pub env_override: Option<&'static str>,
    /// Copy-pasteable remedies, most-likely-correct first. Rendered verbatim
    /// by `--doctor` (after its own `Install: ` prefix), by the liveness
    /// probe's `remedy`, and by [`BackendInstall::not_installed`].
    pub install_hints: &'static [&'static str],
}

/// The env var [`crate::supervisor::SupervisorConfig::local_camoufox`] reads to
/// override the sidecar program. Named here so the probe, the doctor and the
/// refusal all quote the variable the supervisor actually consults.
pub const CAMOUFOX_SIDECAR_ENV: &str = "WAYLAND_CAMOUFOX_BIN";

/// The program name the sidecar's npm package installs onto `PATH`.
pub const CAMOUFOX_SIDECAR_PROGRAM: &str = "camofox-browser";

/// The npm package that provides [`CAMOUFOX_SIDECAR_PROGRAM`].
pub const CAMOUFOX_SIDECAR_PACKAGE: &str = "@askjo/camofox-browser";

/// The Camoufox sidecar — the primary backend, compiled into every build.
pub const CAMOUFOX: BackendInstall = BackendInstall {
    backend: "Camoufox sidecar",
    programs: &[CAMOUFOX_SIDECAR_PROGRAM],
    env_override: Some(CAMOUFOX_SIDECAR_ENV),
    install_hints: &[
        "npm install -g @askjo/camofox-browser",
        "set WAYLAND_CAMOUFOX_BIN to an existing camofox-browser executable (if already installed)",
    ],
};

impl BackendInstall {
    /// The program this backend will actually try to run: the operator's env
    /// override when they set one, else the first candidate name.
    pub fn configured_program(&self) -> String {
        if let Some(key) = self.env_override
            && let Ok(raw) = std::env::var(key)
            && !raw.trim().is_empty()
        {
            return raw.trim().to_string();
        }
        self.programs
            .first()
            .copied()
            .unwrap_or_default()
            .to_string()
    }

    /// Resolve the backend's program **without executing it** (PATHEXT-aware
    /// on Windows via `which`). `Some` means installed.
    pub fn resolve(&self) -> Option<PathBuf> {
        let configured = self.configured_program();
        if !configured.is_empty()
            && let Ok(path) = which::which(&configured)
        {
            return Some(path);
        }
        self.programs
            .iter()
            .find_map(|program| which::which(program).ok())
    }

    /// The install instruction as one line, for surfaces that carry a single
    /// `remedy` string.
    pub fn remedy(&self) -> String {
        self.install_hints.join(", or ")
    }

    /// The one message for "this backend is not installed".
    ///
    /// `program` is what the caller would have run — passed in rather than
    /// re-derived, so the message can never name a different program than the
    /// one the caller actually failed to find. `probed_url` is the sidecar URL
    /// that was tried, when one was.
    pub fn not_installed(&self, program: &str, probed_url: Option<&str>) -> String {
        let probed = match probed_url {
            Some(url) => format!(", and nothing answered {url}"),
            None => String::new(),
        };
        let mut out = format!(
            "The browser backend is not installed. Core runs the {}; `{program}` does not \
             resolve on PATH{probed}.",
            self.backend
        );
        for (i, hint) in self.install_hints.iter().enumerate() {
            let lead = if i == 0 { "Fix:" } else { "Or: " };
            out.push_str(&format!("\n{lead} {hint}"));
        }
        out
    }
}

/// Every locally installed backend compiled into THIS build, in the order the
/// provider selects them.
///
/// Browserbase is deliberately absent: it is a cloud backend with nothing to
/// install locally, and it is reported by its own credential check.
pub fn compiled_backends() -> Vec<&'static BackendInstall> {
    vec![&CAMOUFOX]
}

/// The first compiled-in backend that is actually installed, if any.
pub fn resolve_any() -> Option<(&'static BackendInstall, PathBuf)> {
    compiled_backends()
        .into_iter()
        .find_map(|backend| backend.resolve().map(|path| (backend, path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped build must not advertise a backend it did not compile.
    /// This is the assertion that would have caught `--doctor` telling every
    /// Linux user to `apt install chromium-browser`.
    #[test]
    fn no_compiled_backend_recommends_uncompiled_software() {
        for backend in compiled_backends() {
            for hint in backend.install_hints {
                let hint = hint.to_ascii_lowercase();
                assert!(
                    !hint.contains("chromium") && !hint.contains("chrome"),
                    "no build compiles a Chromium backend, so no remedy may tell the \
                     operator to install one: {hint}"
                );
            }
        }
    }

    /// Camoufox is the primary and is compiled in unconditionally, so it must
    /// always be offered, and offered first.
    #[test]
    fn camoufox_is_always_compiled_in_and_first() {
        let backends = compiled_backends();
        assert_eq!(
            backends.first().map(|b| b.backend),
            Some("Camoufox sidecar")
        );
    }

    /// The remedy has to name the package that provides the program AND the
    /// env var the supervisor reads — those are the two things an operator can
    /// act on, and naming a third variable is the defect gh#491 reports.
    #[test]
    fn the_camoufox_remedy_names_the_package_and_the_supervisors_env_var() {
        let remedy = CAMOUFOX.remedy();
        assert!(remedy.contains(CAMOUFOX_SIDECAR_PACKAGE), "{remedy}");
        assert!(remedy.contains(CAMOUFOX_SIDECAR_ENV), "{remedy}");
        assert!(
            !remedy.contains("CAMOUFOX_EXECUTABLE_PATH"),
            "the install remedy must not name the pref-directory override: {remedy}"
        );
    }

    /// The env override is what the supervisor spawns, so it must be what the
    /// probe and the doctor look for too.
    #[test]
    fn the_env_override_decides_the_configured_program() {
        // Serialised implicitly: this is the only test that touches the var.
        let key = CAMOUFOX_SIDECAR_ENV;
        let prior = std::env::var_os(key);
        unsafe { std::env::set_var(key, "/opt/custom/camofox-browser") };
        assert_eq!(CAMOUFOX.configured_program(), "/opt/custom/camofox-browser");
        unsafe { std::env::set_var(key, "   ") };
        assert_eq!(
            CAMOUFOX.configured_program(),
            CAMOUFOX_SIDECAR_PROGRAM,
            "a blank override must not become the program name"
        );
        match prior {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }

    /// The message must name the program the CALLER failed to find, not one
    /// re-derived here — otherwise an operator with the env var set is told
    /// about a program they never configured.
    #[test]
    fn not_installed_quotes_the_callers_program_and_the_probed_url() {
        let msg =
            CAMOUFOX.not_installed("/opt/x/camofox-browser", Some("http://127.0.0.1:9/health"));
        assert!(msg.contains("/opt/x/camofox-browser"), "{msg}");
        assert!(msg.contains("http://127.0.0.1:9/health"), "{msg}");
        assert!(msg.contains(CAMOUFOX_SIDECAR_PACKAGE), "{msg}");
        assert!(msg.contains(CAMOUFOX_SIDECAR_ENV), "{msg}");
    }
}
