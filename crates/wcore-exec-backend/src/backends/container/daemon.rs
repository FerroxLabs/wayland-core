//! Probe the daemon's operating system, independent of the client's host.

use crate::contract::{Availability, ProbeBasis};

#[derive(serde::Deserialize)]
struct ServerVersion {
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "Os")]
    os: String,
}

fn classify_version_ping(success: bool, stdout: &[u8], stderr: &[u8]) -> Result<String, String> {
    if !success {
        return Err(format!(
            "docker daemon refused the version ping: {}",
            String::from_utf8_lossy(stderr).trim()
        ));
    }
    let server: ServerVersion = serde_json::from_slice(stdout)
        .map_err(|e| format!("docker daemon returned an invalid server version response: {e}"))?;
    if server.version.trim().is_empty() || server.os.trim().is_empty() {
        return Err("docker daemon returned an empty server version or operating system".into());
    }
    if server.os != "linux" {
        return Err(format!(
            "container backend requires a Linux container daemon; server {} reports operating system {}",
            server.version, server.os
        ));
    }
    Ok(server.version)
}

/// The backend and its Linux-image fixtures share this prerequisite. A Windows
/// client connected to a Linux daemon is eligible; a Windows-container daemon
/// cannot execute this backend's Linux image and `/task` workspace.
///
/// The five-second bound also covers an unreachable daemon.
pub async fn linux_daemon_availability() -> Availability {
    let mut command = wcore_config::shell::shell_command_argv(
        "docker",
        &["version", "--format", "{{json .Server}}"],
    );
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let result =
        match tokio::time::timeout(std::time::Duration::from_secs(5), command.output()).await {
            Err(_) => Err("docker daemon did not answer a version ping within 5s".into()),
            Ok(Err(e)) => Err(format!("docker client could not be launched: {e}")),
            Ok(Ok(output)) => {
                classify_version_ping(output.status.success(), &output.stdout, &output.stderr)
            }
        };
    match result {
        Ok(version) => Availability::up(
            ProbeBasis::DaemonPing,
            format!("Linux container daemon answered a version ping: server {version}"),
        ),
        Err(detail) => Availability::down(ProbeBasis::DaemonPing, detail),
    }
}

#[cfg(test)]
mod tests {
    use super::classify_version_ping;

    #[test]
    fn a_linux_daemon_is_eligible_independent_of_the_client_platform() {
        assert_eq!(
            classify_version_ping(
                true,
                br#"{"Version":"29.2.1","Os":"linux","Arch":"amd64"}"#,
                b""
            ),
            Ok("29.2.1".into())
        );
    }

    #[test]
    fn a_windows_daemon_is_explicitly_unavailable_despite_answering_the_ping() {
        let error = classify_version_ping(true, br#"{"Version":"29.1.5","Os":"windows"}"#, b"")
            .unwrap_err();
        assert!(
            error.contains("requires a Linux container daemon"),
            "{error}"
        );
        assert!(error.contains("windows"), "{error}");
    }

    #[test]
    fn missing_malformed_and_unknown_server_platforms_are_not_available() {
        for response in [
            "",
            "null",
            "not json",
            r#"{"Version":"29.2.1"}"#,
            r#"{"Version":"29.2.1","Os":""}"#,
            r#"{"Version":"","Os":"linux"}"#,
            r#"{"Version":"29.2.1","Os":"unknown"}"#,
        ] {
            assert!(
                classify_version_ping(true, response.as_bytes(), b"").is_err(),
                "{response}"
            );
        }
    }

    #[test]
    fn a_failed_daemon_round_trip_cannot_be_made_available_by_stdout() {
        let error = classify_version_ping(
            false,
            br#"{"Version":"29.2.1","Os":"linux"}"#,
            b"cannot connect to daemon",
        )
        .unwrap_err();
        assert!(error.contains("cannot connect to daemon"), "{error}");
    }
}
