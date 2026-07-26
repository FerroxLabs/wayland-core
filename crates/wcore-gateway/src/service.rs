//! Native service management, one implementation per OS family behind one
//! trait.
//!
//! Phase 24 plan 24-01, Tasks 3 and 4. No call site outside this module
//! branches on platform: [`for_this_platform`] is the single selection
//! point, exactly as AGENTS.md requires platform differences to be
//! centralised.
//!
//! # The mechanism per family, and where each came from
//!
//! | family  | mechanism                          | source |
//! |---------|------------------------------------|--------|
//! | macOS   | per-user launch agent              | the repository's existing `templates/cron-daemon/launchd.plist`, retargeted |
//! | Linux   | per-user systemd unit              | the repository's existing `templates/cron-daemon/systemd.service`, retargeted |
//! | Windows | logon-triggered scheduled task     | AUTHORIZED by the Task 3 four-way panel on measured evidence |
//!
//! All three are PER-USER. That symmetry is deliberate and it is the reason
//! the Windows choice is a scheduled task rather than a service control
//! manager registration: a launch agent and a systemd user unit do not run
//! before login and do not require elevation, so the family that matched
//! them is the one that also does not.
//!
//! # Argv mode, always
//!
//! Every external invocation here is built with
//! [`wcore_config::shell::shell_command_argv`] — never a shell string. The
//! service name, the binary path and the home path all cross a trust
//! boundary from the operator (threat T-24-01-01), and in argv mode a shell
//! metacharacter in any of them reaches the child as a literal byte instead
//! of being interpreted.

use std::path::{Path, PathBuf};

/// Windows process creation flags.
///
/// Declared here rather than at each call site so the workspace has ONE
/// definition of what "detached" means. `wcore-cli`'s scheduler daemon
/// consumes these; see the measurement note at that call site.
///
/// `CREATE_BREAKAWAY_FROM_JOB` is the load-bearing flag: Windows OpenSSH
/// reaps session children through a Job Object, and detaching the console
/// and leaving the process group do not leave that job.
pub const DETACHED_PROCESS: u32 = 0x0000_0008;
/// See [`DETACHED_PROCESS`].
pub const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
/// See [`DETACHED_PROCESS`].
pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// What an installer needs to register one gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    /// The profile this gateway hosts. One gateway, one home, one profile.
    pub profile: String,
    /// Absolute path to the binary to run, resolved from the RUNNING
    /// executable rather than from an operator-supplied string.
    pub binary: PathBuf,
    /// The gateway home, already normalised.
    pub home: PathBuf,
}

impl ServiceSpec {
    /// The registered identifier for this spec on any family.
    ///
    /// The profile crosses a trust boundary, so it is validated here rather
    /// than at each family's call site: only ASCII alphanumerics, dash and
    /// underscore survive, which is a subset every one of the three
    /// registries accepts literally.
    pub fn service_name(&self) -> String {
        let sanitised: String = self
            .profile
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        format!("wayland-core-gateway-{sanitised}")
    }
}

/// Whether a registration exists, and whether it is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    NotRegistered,
    Registered,
    Running,
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("service mechanism `{mechanism}` is not available on this platform")]
    Unsupported { mechanism: &'static str },

    #[error("service command failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("service command `{command}` exited with status {status}: {stderr}")]
    Command {
        command: String,
        status: i32,
        stderr: String,
    },
}

/// One implementation per OS family.
pub trait ServiceManager {
    /// The family this manager targets, for the status projection and the
    /// operator's own reading.
    fn family(&self) -> &'static str;

    /// The command line that registers the gateway, as ARGV. Returned
    /// rather than run so it can be asserted on in a test and printed to an
    /// operator before it is executed.
    fn install_argv(&self, spec: &ServiceSpec) -> Vec<String>;

    /// The command line that removes the registration.
    fn uninstall_argv(&self, spec: &ServiceSpec) -> Vec<String>;

    /// The command line that starts the registered gateway.
    fn start_argv(&self, spec: &ServiceSpec) -> Vec<String>;

    /// The command line that stops it.
    fn stop_argv(&self, spec: &ServiceSpec) -> Vec<String>;

    /// The command line that queries it.
    fn status_argv(&self, spec: &ServiceSpec) -> Vec<String>;

    /// The unit/plist/task definition written to disk, when the family
    /// needs one. Windows registers through a command line and needs none.
    fn unit_text(&self, spec: &ServiceSpec) -> Option<String>;

    /// Where [`Self::unit_text`] is written.
    fn unit_path(&self, spec: &ServiceSpec) -> Option<PathBuf>;
}

/// The single platform-selection point in the workspace.
pub fn for_this_platform() -> Box<dyn ServiceManager> {
    #[cfg(target_os = "macos")]
    {
        Box::new(LaunchdManager)
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(SystemdManager)
    }
    #[cfg(windows)]
    {
        Box::new(ScheduledTaskManager)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        Box::new(UnsupportedManager)
    }
}

/// macOS: a per-user launch agent, retargeting the shape the repository
/// already ships for the background scheduler.
#[derive(Debug, Default)]
pub struct LaunchdManager;

impl ServiceManager for LaunchdManager {
    fn family(&self) -> &'static str {
        "launchd"
    }

    fn install_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec![
            "launchctl".into(),
            "load".into(),
            "-w".into(),
            self.unit_path(spec)
                .expect("launchd always has a unit path")
                .to_string_lossy()
                .into_owned(),
        ]
    }

    fn uninstall_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec![
            "launchctl".into(),
            "unload".into(),
            "-w".into(),
            self.unit_path(spec)
                .expect("launchd always has a unit path")
                .to_string_lossy()
                .into_owned(),
        ]
    }

    fn start_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec!["launchctl".into(), "start".into(), spec.service_name()]
    }

    fn stop_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec!["launchctl".into(), "stop".into(), spec.service_name()]
    }

    fn status_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec!["launchctl".into(), "list".into(), spec.service_name()]
    }

    fn unit_text(&self, spec: &ServiceSpec) -> Option<String> {
        Some(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>            <string>{name}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{binary}</string>
    <string>gateway</string>
    <string>run</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>WAYLAND_HOME</key>   <string>{home}</string>
  </dict>
  <key>RunAtLoad</key>        <true/>
  <key>KeepAlive</key>        <true/>
  <key>StandardOutPath</key>  <string>{home}/gateway.log</string>
  <key>StandardErrorPath</key><string>{home}/gateway.log</string>
</dict>
</plist>
"#,
            name = spec.service_name(),
            binary = spec.binary.display(),
            home = spec.home.display(),
        ))
    }

    fn unit_path(&self, spec: &ServiceSpec) -> Option<PathBuf> {
        dirs::home_dir().map(|h| {
            h.join("Library")
                .join("LaunchAgents")
                .join(format!("{}.plist", spec.service_name()))
        })
    }
}

/// Linux: a per-user systemd unit, retargeting the shape the repository
/// already ships for the background scheduler.
#[derive(Debug, Default)]
pub struct SystemdManager;

impl ServiceManager for SystemdManager {
    fn family(&self) -> &'static str {
        "systemd"
    }

    fn install_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec![
            "systemctl".into(),
            "--user".into(),
            "enable".into(),
            format!("{}.service", spec.service_name()),
        ]
    }

    fn uninstall_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec![
            "systemctl".into(),
            "--user".into(),
            "disable".into(),
            format!("{}.service", spec.service_name()),
        ]
    }

    fn start_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec![
            "systemctl".into(),
            "--user".into(),
            "start".into(),
            format!("{}.service", spec.service_name()),
        ]
    }

    fn stop_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec![
            "systemctl".into(),
            "--user".into(),
            "stop".into(),
            format!("{}.service", spec.service_name()),
        ]
    }

    fn status_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec![
            "systemctl".into(),
            "--user".into(),
            "is-active".into(),
            format!("{}.service", spec.service_name()),
        ]
    }

    fn unit_text(&self, spec: &ServiceSpec) -> Option<String> {
        Some(format!(
            "[Unit]\n\
             Description=Wayland Core gateway ({profile})\n\
             After=network.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             Environment=WAYLAND_HOME={home}\n\
             ExecStart={binary} gateway run\n\
             Restart=on-failure\n\
             RestartSec=5\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            profile = spec.profile,
            home = spec.home.display(),
            binary = spec.binary.display(),
        ))
    }

    fn unit_path(&self, spec: &ServiceSpec) -> Option<PathBuf> {
        dirs::config_dir().map(|c| {
            c.join("systemd")
                .join("user")
                .join(format!("{}.service", spec.service_name()))
        })
    }
}

/// Windows: a logon-triggered scheduled task.
///
/// AUTHORIZED by the Task 3 four-way panel (4/4) on measured evidence, and
/// the alternatives were excluded by measurement rather than by argument:
///
/// - the service control manager registered but its central property —
///   a surviving process — was never demonstrated, because no binary in
///   this workspace answers the service control handshake (`sc start`
///   returned 1053);
/// - a scheduled task registered, ran, and its process was still advancing
///   its heartbeat from a LATER, SEPARATE session after the registration
///   had already been deleted.
///
/// OPEN RISK, carried to 24-04 rather than closed here: task
/// restart-on-failure is capped in count and delayed in time, and is
/// genuinely weaker than a service control manager recovery policy.
/// Criterion 5 requires the PLATFORM to bring the runtime back after a hard
/// kill; nothing measured in 24-01 shows that it does.
#[derive(Debug, Default)]
pub struct ScheduledTaskManager;

impl ServiceManager for ScheduledTaskManager {
    fn family(&self) -> &'static str {
        "schtasks"
    }

    fn install_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        // ARGV mode. The binary path and home cross a trust boundary from
        // the operator; in argv mode a metacharacter in either reaches the
        // child as a literal byte rather than being interpreted (T-24-01-01).
        vec![
            "schtasks".into(),
            "/create".into(),
            "/tn".into(),
            spec.service_name(),
            "/tr".into(),
            format!("\"{}\" gateway run", spec.binary.display()),
            "/sc".into(),
            "onlogon".into(),
            "/f".into(),
        ]
    }

    fn uninstall_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec![
            "schtasks".into(),
            "/delete".into(),
            "/tn".into(),
            spec.service_name(),
            "/f".into(),
        ]
    }

    fn start_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec![
            "schtasks".into(),
            "/run".into(),
            "/tn".into(),
            spec.service_name(),
        ]
    }

    fn stop_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        vec![
            "schtasks".into(),
            "/end".into(),
            "/tn".into(),
            spec.service_name(),
        ]
    }

    fn status_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        // `/fo list /v` so the parse can anchor on a FIELD NAME rather than
        // a column offset — the option's own recorded cost is that this is a
        // command-line contract rather than a typed interface, and anchoring
        // on names is how that cost is paid down.
        vec![
            "schtasks".into(),
            "/query".into(),
            "/tn".into(),
            spec.service_name(),
            "/v".into(),
            "/fo".into(),
            "list".into(),
        ]
    }

    fn unit_text(&self, _spec: &ServiceSpec) -> Option<String> {
        // Windows registers through a command line; there is no on-disk
        // unit to write.
        None
    }

    fn unit_path(&self, _spec: &ServiceSpec) -> Option<PathBuf> {
        None
    }
}

/// Every other target. Named refusal, never a silent success.
#[derive(Debug, Default)]
pub struct UnsupportedManager;

impl ServiceManager for UnsupportedManager {
    fn family(&self) -> &'static str {
        "unsupported"
    }
    fn install_argv(&self, _: &ServiceSpec) -> Vec<String> {
        Vec::new()
    }
    fn uninstall_argv(&self, _: &ServiceSpec) -> Vec<String> {
        Vec::new()
    }
    fn start_argv(&self, _: &ServiceSpec) -> Vec<String> {
        Vec::new()
    }
    fn stop_argv(&self, _: &ServiceSpec) -> Vec<String> {
        Vec::new()
    }
    fn status_argv(&self, _: &ServiceSpec) -> Vec<String> {
        Vec::new()
    }
    fn unit_text(&self, _: &ServiceSpec) -> Option<String> {
        None
    }
    fn unit_path(&self, _: &ServiceSpec) -> Option<PathBuf> {
        None
    }
}

/// Resolve the binary to register from the RUNNING executable, never from
/// an operator-supplied string (threat T-24-01-01).
pub fn running_binary() -> Result<PathBuf, ServiceError> {
    Ok(std::env::current_exe()?)
}

/// Whether `p` is an absolute path. Registration refuses a relative one:
/// a service that resolves its own binary against a working directory it
/// does not control is a substitution waiting to happen.
pub fn is_registerable_binary(p: &Path) -> bool {
    p.is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ServiceSpec {
        ServiceSpec {
            profile: "default".into(),
            binary: PathBuf::from("/opt/wayland/bin/wayland-core"),
            home: PathBuf::from("/home/op/.wayland"),
        }
    }

    #[test]
    fn a_hostile_profile_name_cannot_escape_the_service_identifier() {
        let s = ServiceSpec {
            profile: "a b;rm -rf /$(x)`y`&z|w".into(),
            ..spec()
        };
        let name = s.service_name();
        for bad in [' ', ';', '$', '(', ')', '`', '&', '|', '/'] {
            assert!(
                !name.contains(bad),
                "service identifier must not carry {bad:?}: {name}"
            );
        }
        assert!(name.starts_with("wayland-core-gateway-"));
    }

    #[test]
    fn every_family_produces_a_non_empty_argv_for_every_verb() {
        let s = spec();
        let managers: Vec<Box<dyn ServiceManager>> = vec![
            Box::new(LaunchdManager),
            Box::new(SystemdManager),
            Box::new(ScheduledTaskManager),
        ];
        for m in managers {
            for (verb, argv) in [
                ("install", m.install_argv(&s)),
                ("uninstall", m.uninstall_argv(&s)),
                ("start", m.start_argv(&s)),
                ("stop", m.stop_argv(&s)),
                ("status", m.status_argv(&s)),
            ] {
                assert!(!argv.is_empty(), "{} has no argv for {verb}", m.family());
                // ARGV mode means the program is argv[0] and every other
                // element is a separate entry. A single element carrying
                // spaces and metacharacters would mean somebody rebuilt a
                // shell string here.
                assert!(
                    !argv[0].contains(' '),
                    "{} {verb} argv[0] must be a bare program name, got {:?}",
                    m.family(),
                    argv[0]
                );
            }
        }
    }

    #[test]
    fn the_windows_manager_writes_no_unit_and_the_unix_families_do() {
        let s = spec();
        assert!(ScheduledTaskManager.unit_text(&s).is_none());
        assert!(ScheduledTaskManager.unit_path(&s).is_none());
        assert!(SystemdManager.unit_text(&s).is_some());
        assert!(LaunchdManager.unit_text(&s).is_some());
    }

    #[test]
    fn both_unix_units_are_per_user_and_carry_the_home_and_binary() {
        let s = spec();
        let sd = SystemdManager.unit_text(&s).unwrap();
        assert!(
            sd.contains("WantedBy=default.target"),
            "must be a USER unit"
        );
        assert!(sd.contains("/home/op/.wayland"));
        assert!(sd.contains("/opt/wayland/bin/wayland-core"));

        let la = LaunchdManager.unit_text(&s).unwrap();
        assert!(la.contains("<key>Label</key>"));
        assert!(la.contains("/home/op/.wayland"));
        assert!(la.contains("/opt/wayland/bin/wayland-core"));

        // Per-user placement, not a system-wide daemon directory.
        if let Some(p) = LaunchdManager.unit_path(&s) {
            assert!(p.to_string_lossy().contains("LaunchAgents"));
        }
    }

    #[test]
    fn the_windows_task_is_registered_at_logon_and_forced() {
        let argv = ScheduledTaskManager.install_argv(&spec());
        assert_eq!(argv[0], "schtasks");
        assert!(argv.iter().any(|a| a == "onlogon"));
        assert!(argv.iter().any(|a| a == "/f"));
    }

    #[test]
    fn a_relative_binary_is_not_registerable() {
        assert!(!is_registerable_binary(Path::new("wayland-core")));
        assert!(!is_registerable_binary(Path::new("./wayland-core")));

        // The absolute case MUST be written per-family. `/opt/...` has no
        // prefix component on Windows, so `is_absolute()` reports false
        // there and this assertion fails — measured on SEANDESKTOP
        // 2026-07-26, where it was the only red in the suite. AGENTS.md
        // names exactly this: a hardcoded Unix path is fine for pure string
        // work, and needs a per-platform variant the moment it reaches
        // `is_absolute()`.
        #[cfg(unix)]
        assert!(is_registerable_binary(Path::new("/opt/x/wayland-core")));
        #[cfg(windows)]
        assert!(is_registerable_binary(Path::new(
            r"C:\Program Files\Wayland\wayland-core.exe"
        )));

        // A Windows drive-RELATIVE path (`C:wayland-core`) is a real trap:
        // it looks absolute and is not. It resolves against the drive's
        // current directory, which a service does not control — precisely
        // the substitution `is_registerable_binary` exists to refuse.
        #[cfg(windows)]
        assert!(!is_registerable_binary(Path::new("C:wayland-core.exe")));
    }

    #[test]
    fn the_detach_flags_are_the_measured_set() {
        // Guards the measurement: CREATE_BREAKAWAY_FROM_JOB is the flag
        // that actually leaves the OpenSSH job object, and losing it would
        // silently restore the defect the probe caught.
        assert_eq!(DETACHED_PROCESS, 0x0000_0008);
        assert_eq!(CREATE_NEW_PROCESS_GROUP, 0x0000_0200);
        assert_eq!(CREATE_BREAKAWAY_FROM_JOB, 0x0100_0000);
    }
}
