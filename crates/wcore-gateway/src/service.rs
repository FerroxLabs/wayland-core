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
        format!("wayland-core-gateway-{}", self.sanitised_profile())
    }

    /// The profile as it is safe to write into a unit, a plist argv or a
    /// scheduled-task command line.
    ///
    /// ONE sanitiser, used by the service identifier AND by the `--profile`
    /// argument every generated unit now passes to `gateway run`. They must
    /// agree: a unit registered as `wayland-core-gateway-my-profile` whose
    /// runtime reports a different profile string is precisely the
    /// misreport the live Linux journey caught (F24-B-H1). Only ASCII
    /// alphanumerics, dash and underscore survive, which is a subset every
    /// one of the three registries — and every one of the three unit
    /// formats — accepts literally, so an operator profile carrying a space
    /// cannot split a systemd ExecStart into two tokens.
    pub fn sanitised_profile(&self) -> String {
        self.profile
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect()
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
    /// needs one.
    fn unit_text(&self, spec: &ServiceSpec) -> Option<String>;

    /// Where [`Self::unit_text`] is written.
    fn unit_path(&self, spec: &ServiceSpec) -> Option<PathBuf>;

    /// Whether [`Self::unit_path`] is this family's REGISTRATION RECORD — a
    /// file the platform itself reads to decide the service exists.
    ///
    /// F24-J-H3. This was previously inferred from `unit_path().is_some()`,
    /// which was correct only while Windows was the sole family without a
    /// unit file. Windows now writes one, but Task Scheduler COPIES it into
    /// its own store at `/create` time and never reads it again — the file is
    /// an import artifact, not a registration. Inferring from its presence
    /// would report `Registered` for a task deleted out of band, which is
    /// exactly the misreport F24-B-H2 closed for systemd. A family that
    /// answers `false` is asked its query verb instead.
    fn unit_is_registration_record(&self) -> bool {
        true
    }
}

/// Escape a value for XML ELEMENT TEXT.
///
/// The binary path and the gateway home cross a trust boundary from the
/// operator (T-24-01-01) and, on Windows, they are now interpolated into a
/// Task Scheduler XML document rather than only into an argv. A path
/// containing `&` or `<` would otherwise produce a malformed document —
/// `schtasks` rejects it with `The task XML is malformed`, which is a refusal
/// rather than a breach, but a `]]>`-shaped value in a less careful document
/// shape is not. Escaping at the single point of interpolation is the
/// invariant, not the fact that today's rejection happens to be safe.
fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
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
    <string>--profile</string>
    <string>{profile}</string>
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
            profile = spec.sanitised_profile(),
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
             ExecStart={binary} gateway run --profile {profile}\n\
             Restart=on-failure\n\
             RestartSec=5\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            profile = spec.sanitised_profile(),
            home = spec.home.display(),
            binary = spec.binary.display(),
        ))
    }

    /// `<os-native config root>/systemd/user/<name>.service`.
    ///
    /// Routed through `wcore_config::config::os_native_config_root()` rather
    /// than a raw `dirs::config_dir()` so the workspace keeps ONE call site for
    /// the native root (the hermeticity audit, F-010/#270, gates exactly that).
    /// It is deliberately NOT `wayland_config_dir()`: systemd's user manager
    /// only scans `$XDG_CONFIG_HOME/systemd/user` (else `~/.config/systemd/user`),
    /// so a unit written under `$WAYLAND_HOME` would never be discovered and
    /// `install` would register nothing while reporting success. Hermeticity is
    /// preserved by the unit's own `Environment=WAYLAND_HOME=` line — the unit
    /// is a pointer INTO the hermetic home, not state inside it.
    fn unit_path(&self, spec: &ServiceSpec) -> Option<PathBuf> {
        wcore_config::config::os_native_config_root().map(|c| {
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
/// # Recovery after a hard kill (F24-J-H3) — measured, not assumed
///
/// 24-01 carried this forward as an OPEN RISK in its own words: *"Criterion 5
/// requires the PLATFORM to bring the runtime back after a hard kill; nothing
/// measured in 24-01 shows that it does."* 24-C5 measured it on the real box,
/// and the answer under the original `/create /sc onlogon` registration was
/// **no** — `schtasks /create /sc onlogon` sets no recovery policy at all, so
/// after `taskkill /F` the task returned to `Ready` with `Last Result: 1` and
/// stayed there. The registration is therefore built from an XML document
/// (`/create /xml`), which is the only `schtasks` path that can express one.
///
/// Three things about that document were established by measurement on
/// Windows 11 26100, and each contradicts the obvious guess:
///
/// 1. **`<RestartOnFailure>` alone does NOT recover this service.** It
///    registers, and Task Scheduler's own `/query /xml` reads it back, and
///    after a `taskkill /F` of a task started on demand the runtime was still
///    down 3m20s later against a `PT1M` interval. It is kept below because it
///    covers the *fails to start* case, but it is NOT what recovers a killed
///    process, and a gate asserting only its presence in the XML would have
///    passed over a service that never came back.
/// 2. **A repetition trigger is what recovers it.** A `<TimeTrigger>` with a
///    one-minute `<Repetition>` and `MultipleInstancesPolicy=IgnoreNew` is a
///    supervisor: every minute the platform tries to start the task, an
///    already-running instance makes that a no-op, and a dead one is
///    replaced. Measured: pid killed at 21:21:25 with **no** manual start,
///    platform-started replacement pid at 21:22:01, and a single instance
///    still (`INSTANCE_COUNT=1`) minutes of repetitions later.
/// 3. **The declaration must say `encoding="UTF-16"`.** `schtasks` rejects
///    `encoding="UTF-8"` with `ERROR: unable to switch the encoding` even
///    when the bytes genuinely are UTF-8; with the UTF-16 declaration the
///    same UTF-8 bytes are accepted. The file this crate writes is a Rust
///    `String`, so it is UTF-8; the declaration is what Windows insists on.
///
/// No `<Principals>` block is emitted, and that is also measured rather than
/// stylistic: `schtasks` supplies the invoking user with `Logon Mode:
/// Interactive only` when the element is absent, which is the per-user,
/// non-elevated identity this module wants, while an explicit
/// `<UserId>%USERDOMAIN%\%USERNAME%</UserId>` is REJECTED on a workgroup
/// machine (`No mapping between account names and security IDs was done`,
/// because `USERDOMAIN` is `WORKGROUP`). Deriving the principal from the
/// environment would have failed on every non-domain-joined desktop.
///
/// KNOWN DIVERGENCE, deliberate: while the task is registered, `gateway stop`
/// (`schtasks /end`) is not durable — the repetition restarts the runtime
/// within a minute. That is the same trade macOS already makes, where
/// `KeepAlive` undoes `launchctl stop`; systemd is the outlier in
/// distinguishing an explicit stop from a failure. `uninstall` removes the
/// task and therefore the supervisor, so the drain-then-uninstall path is
/// unaffected.
#[derive(Debug, Default)]
pub struct ScheduledTaskManager;

impl ServiceManager for ScheduledTaskManager {
    fn family(&self) -> &'static str {
        "schtasks"
    }

    fn install_argv(&self, spec: &ServiceSpec) -> Vec<String> {
        // ARGV mode. The task name and the XML path cross a trust boundary
        // from the operator; in argv mode a metacharacter in either reaches
        // the child as a literal byte rather than being interpreted
        // (T-24-01-01).
        //
        // F24-J-H3: `/xml` rather than `/sc onlogon`. The recovery policy the
        // criterion requires cannot be expressed on the `/sc` command line at
        // all — see the type's own documentation for what was measured.
        vec![
            "schtasks".into(),
            "/create".into(),
            "/tn".into(),
            spec.service_name(),
            "/xml".into(),
            self.unit_path(spec)
                .expect("the scheduled-task family always has a unit path")
                .to_string_lossy()
                .into_owned(),
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

    fn unit_text(&self, spec: &ServiceSpec) -> Option<String> {
        // F24-J-H1 is preserved verbatim through the `<Arguments>` element:
        // Task Scheduler has NO mechanism for setting an environment variable
        // on a task, while the launchd plist carries
        // `EnvironmentVariables/WAYLAND_HOME` and the systemd unit carries
        // `Environment=WAYLAND_HOME=`. Without `--home` the task ran the
        // runtime against the DEFAULT home rather than the one it was
        // installed for. The home is an ARGUMENT, never a `cmd /c "set ..."`
        // wrapper, which would interpolate an operator path into a shell
        // string — the injection shape AGENTS.md forbids.
        //
        // `<Command>` and `<Arguments>` are separate elements, so the binary
        // path is not part of any string Windows has to re-split.
        Some(format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Wayland Core gateway ({profile})</Description>
  </RegistrationInfo>
  <Settings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <Priority>7</Priority>
  </Settings>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
    </LogonTrigger>
    <TimeTrigger>
      <StartBoundary>2000-01-01T00:00:00</StartBoundary>
      <Enabled>true</Enabled>
      <Repetition>
        <Interval>PT1M</Interval>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
    </TimeTrigger>
  </Triggers>
  <Actions Context="Author">
    <Exec>
      <Command>{binary}</Command>
      <Arguments>gateway run --profile {profile} --home "{home}"</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
            profile = xml_escape(&spec.sanitised_profile()),
            binary = xml_escape(&spec.binary.display().to_string()),
            home = xml_escape(&spec.home.display().to_string()),
        ))
    }

    /// `<gateway home>/<service name>.task.xml`.
    ///
    /// Inside the hermetic home, unlike the two Unix families, because this
    /// file is an IMPORT ARTIFACT rather than a registration: Task Scheduler
    /// copies it into its own store at `/create` time and never reads the
    /// path again. Nothing outside `$WAYLAND_HOME` needs to scan for it, so
    /// nothing outside `$WAYLAND_HOME` is written. See
    /// [`ServiceManager::unit_is_registration_record`].
    fn unit_path(&self, spec: &ServiceSpec) -> Option<PathBuf> {
        Some(spec.home.join(format!("{}.task.xml", spec.service_name())))
    }

    fn unit_is_registration_record(&self) -> bool {
        false
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
    fn all_three_families_write_a_unit_but_only_two_of_them_are_registrations() {
        // F24-J-H3. Windows gained a unit file, and the distinction that
        // used to be carried by `unit_path().is_none()` had to move somewhere
        // explicit or `is_registered` would silently start answering from a
        // file Task Scheduler never reads.
        let s = spec();
        assert!(ScheduledTaskManager.unit_text(&s).is_some());
        assert!(ScheduledTaskManager.unit_path(&s).is_some());
        assert!(SystemdManager.unit_text(&s).is_some());
        assert!(LaunchdManager.unit_text(&s).is_some());

        assert!(SystemdManager.unit_is_registration_record());
        assert!(LaunchdManager.unit_is_registration_record());
        assert!(
            !ScheduledTaskManager.unit_is_registration_record(),
            "the schtasks XML is an import artifact, not a registration record"
        );

        // The import artifact belongs inside the hermetic home; the two Unix
        // units are pointers INTO the home from a directory their platform
        // scans, so they do not.
        let p = ScheduledTaskManager.unit_path(&s).unwrap();
        assert!(
            p.starts_with(&s.home),
            "the task XML must live inside the gateway home, got {}",
            p.display()
        );
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
    fn the_windows_task_is_registered_from_xml_at_logon_and_forced() {
        let s = spec();
        let argv = ScheduledTaskManager.install_argv(&s);
        assert_eq!(argv[0], "schtasks");
        assert!(argv.iter().any(|a| a == "/f"));
        // F24-J-H3: `/sc onlogon` cannot express a recovery policy at all, so
        // the registration is an XML import. Going back to `/sc` is exactly
        // the regression this asserts against.
        assert!(
            argv.iter().any(|a| a == "/xml"),
            "the registration must be an XML import: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "/sc"),
            "a `/sc` registration carries no recovery policy: {argv:?}"
        );
        let xml_path = ScheduledTaskManager.unit_path(&s).unwrap();
        assert!(
            argv.iter().any(|a| a == &xml_path.to_string_lossy()),
            "the argv must name the file the caller writes: {argv:?}"
        );
        // The logon trigger did not go away; it moved into the document.
        let xml = ScheduledTaskManager.unit_text(&s).unwrap();
        assert!(xml.contains("<LogonTrigger>"), "{xml}");
    }

    #[test]
    fn the_windows_task_recovers_through_a_repetition_supervisor_not_restart_on_failure() {
        // MEASURED on Windows 11 26100, and the measurement is the reason
        // this test is shaped the way it is: a task carrying ONLY
        // `<RestartOnFailure>` registered, read back through Task
        // Scheduler's own `/query /xml`, and then did NOT bring the runtime
        // back — 3m20s after `taskkill /F` against a `PT1M` interval the
        // status was still `Ready`, `Last Result: 1`. A gate asserting the
        // presence of `<RestartOnFailure>` would have passed a service that
        // stays dead, which is the same self-passing shape as asserting on a
        // file the executor itself wrote.
        //
        // What recovers it is a repetition trigger plus `IgnoreNew`: the
        // platform attempts a start every minute, an already-running
        // instance makes that a no-op, a dead one is replaced. Measured: pid
        // killed 21:21:25, no manual start, platform-started replacement at
        // 21:22:01, single instance still minutes later.
        let xml = ScheduledTaskManager.unit_text(&spec()).unwrap();
        assert!(
            xml.contains("<Repetition>"),
            "no repetition trigger: nothing restarts a killed runtime\n{xml}"
        );
        assert!(
            xml.contains("<Interval>PT1M</Interval>"),
            "the repetition interval is the recovery latency bound\n{xml}"
        );
        assert!(
            xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"),
            "without IgnoreNew the repetition stacks a new gateway every minute\n{xml}"
        );
        assert!(
            xml.contains("<StopAtDurationEnd>false</StopAtDurationEnd>"),
            "the supervisor must not expire\n{xml}"
        );
        // Kept for the fails-to-start case, but never the load-bearing part.
        assert!(xml.contains("<RestartOnFailure>"), "{xml}");
    }

    #[test]
    fn the_windows_task_xml_is_in_the_only_form_schtasks_accepts() {
        // MEASURED, and it contradicts the obvious guess. `schtasks
        // /create /xml` rejects a declaration of `encoding="UTF-8"` with
        // `ERROR: unable to switch the encoding` even when the bytes are
        // genuinely UTF-8; the same bytes are accepted under an
        // `encoding="UTF-16"` declaration. This crate writes a Rust `String`,
        // so the bytes are always UTF-8 and the declaration is the part that
        // has to say UTF-16.
        let xml = ScheduledTaskManager.unit_text(&spec()).unwrap();
        assert!(
            xml.starts_with(r#"<?xml version="1.0" encoding="UTF-16"?>"#),
            "schtasks refuses any other declaration\n{xml}"
        );
        // Also measured: with the element absent, `schtasks` supplies the
        // invoking user and `Logon Mode: Interactive only`, which is the
        // per-user non-elevated identity this module wants. An explicit
        // `%USERDOMAIN%\%USERNAME%` principal is REJECTED on a workgroup
        // machine — `No mapping between account names and security IDs was
        // done`, because USERDOMAIN is `WORKGROUP` — so deriving one from
        // the environment would fail on every non-domain-joined desktop.
        assert!(
            !xml.contains("<Principals>"),
            "an environment-derived principal does not resolve on a workgroup machine\n{xml}"
        );
    }

    #[test]
    fn a_hostile_path_cannot_break_out_of_the_windows_task_document() {
        // The binary path and the home cross a trust boundary from the
        // operator, and on Windows they are now interpolated into XML rather
        // than only into an argv.
        let s = ServiceSpec {
            profile: "default".into(),
            binary: PathBuf::from(r"C:\a&b\<x>\wayland-core.exe"),
            home: PathBuf::from(r"C:\home\o'p\a&b"),
        };
        let xml = ScheduledTaskManager.unit_text(&s).unwrap();
        assert!(!xml.contains("a&b"), "an unescaped & survived:\n{xml}");
        assert!(
            !xml.contains("<x>"),
            "an unescaped element survived:\n{xml}"
        );
        assert!(xml.contains("a&amp;b"), "the & was not escaped:\n{xml}");
        assert!(
            xml.contains("&lt;x&gt;"),
            "the brackets were not escaped:\n{xml}"
        );
        assert!(
            xml.contains("o&apos;p"),
            "the quote was not escaped:\n{xml}"
        );

        // The document must still be the shape Task Scheduler parses: one
        // `<Exec>`, and the escaping must not have eaten a real element.
        assert_eq!(xml.matches("<Exec>").count(), 1, "{xml}");
        assert_eq!(xml.matches("</Task>").count(), 1, "{xml}");
    }

    #[test]
    fn every_family_carries_the_home_into_the_registration_it_writes() {
        // F24-J-H1, and it is a THREE-family assertion on purpose. Windows was
        // the only family that carried no home, and it was the only family
        // without a test that said it had to. Measured live on the real box:
        // the task launched the runtime, the runtime resolved the DEFAULT home,
        // and `gateway status --profile f24j` answered `stopped` with a null
        // pid about a process that was in the task list.
        let s = spec();
        let home = s.home.display().to_string();

        let systemd = SystemdManager
            .unit_text(&s)
            .expect("systemd always writes a unit");
        assert!(
            systemd.contains(&home),
            "systemd unit lost the home:\n{systemd}"
        );

        let launchd = LaunchdManager
            .unit_text(&s)
            .expect("launchd always writes a unit");
        assert!(
            launchd.contains(&home),
            "launchd plist lost the home:\n{launchd}"
        );

        // Windows has no environment-variable mechanism on a task, so the
        // home has to reach the runtime as an ARGUMENT or it reaches it
        // nowhere. Since F24-J-H3 that argument lives in the XML the
        // registration imports rather than in a `/tr` command string.
        let task = ScheduledTaskManager
            .unit_text(&s)
            .expect("the scheduled-task family always writes a document");
        assert!(task.contains(&home), "the task XML lost the home:\n{task}");
        assert!(
            task.contains("--home"),
            "the task XML must pass --home explicitly:\n{task}"
        );
        // And a shell wrapper is still forbidden: `cmd /c "set WAYLAND_HOME=…"`
        // would interpolate an operator path into a shell string.
        assert!(
            !task.contains("cmd /c") && !task.contains("cmd.exe"),
            "the home must not be carried by a shell wrapper:\n{task}"
        );
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
    fn every_family_passes_the_profile_to_the_runtime_it_registers() {
        // F24-B-H1, found by the LIVE Linux journey and not by any test that
        // existed before it. The units invoked `gateway run` with no
        // `--profile`, so the runtime resolved `default` from the
        // environment while the registration was named for profile `f24b`.
        // `gateway status --profile f24b` then printed `profile: default` —
        // a status verb contradicting the service identity it was asked
        // about, which is Criterion 1's profile-isolation clause failing.
        //
        // Goes red if any family stops passing the profile, and red if a
        // family passes the RAW profile instead of the sanitised one.
        let hostile = ServiceSpec {
            profile: "a b;rm -rf /".into(),
            binary: PathBuf::from("/opt/x/wayland-core"),
            home: PathBuf::from("/home/op/.wayland"),
        };
        let sane = hostile.sanitised_profile();
        assert!(!sane.contains(' '), "the sanitised profile is one token");

        let managers: Vec<Box<dyn ServiceManager>> = vec![
            Box::new(LaunchdManager),
            Box::new(SystemdManager),
            Box::new(ScheduledTaskManager),
        ];
        for m in managers {
            let rendered = match m.unit_text(&hostile) {
                Some(t) => t,
                None => m.install_argv(&hostile).join(" "),
            };
            assert!(
                rendered.contains("--profile"),
                "{} must pass --profile to `gateway run`: {rendered}",
                m.family()
            );
            assert!(
                rendered.contains(&sane),
                "{} must pass the SANITISED profile {sane:?}: {rendered}",
                m.family()
            );
            assert!(
                !rendered.contains("rm -rf"),
                "{} leaked the raw profile into a unit: {rendered}",
                m.family()
            );
        }
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
