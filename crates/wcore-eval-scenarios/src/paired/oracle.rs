use super::{Family, PairedTask};
use crate::process_tree::ProcessTree;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::io::AsyncReadExt;

/// Execute independent fixture tests against the actual candidate artifacts
/// under the same process containment used by candidate runs, without keys.
pub(super) async fn check(
    task: &PairedTask,
    workspace: &Path,
    final_text: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(workspace.is_dir(), "paired workspace is absent");
    let mutable: &[&str] = match task.family {
        Family::BoundedCodeChange | Family::LongSessionConstraintRetention => &["shipping.py"],
        Family::MultiFileChange => &["config.py", "client.py", "README.md"],
        Family::ToolSkillDiscovery => &["label.txt"],
        Family::InterruptedExecutionRecovery => &["recovery-report.txt"],
        _ => &[],
    };
    for (path, expected) in &task.files {
        let protected = !mutable.contains(&path.as_str());
        if protected {
            let actual = workspace.join(path);
            anyhow::ensure!(
                !std::fs::symlink_metadata(&actual)?.file_type().is_symlink(),
                "protected fixture became a symlink"
            );
            anyhow::ensure!(
                std::fs::read(&actual)? == expected.as_bytes(),
                "protected fixture changed: {path}"
            );
        }
    }
    reject_extra_files(workspace, workspace, task, mutable)?;
    match task.family {
        Family::AnswerWithoutAction => anyhow::ensure!(
            final_text.trim() == format!("{}:46", task.sentinel),
            "answer oracle failed"
        ),
        Family::RepositoryDiagnosis => {
            let answer: serde_json::Value = serde_json::from_str(final_text)?;
            anyhow::ensure!(
                answer["file"] == "shipping.py"
                    && answer["function"] == "fee"
                    && answer["input"] == 50
                    && answer["expected"] == 0
                    && answer["actual"] == 5
                    && answer["sentinel"] == task.sentinel,
                "diagnosis oracle failed"
            );
        }
        Family::BoundedCodeChange
        | Family::MultiFileChange
        | Family::LongSessionConstraintRetention => {
            let test = match task.family {
                Family::MultiFileChange => "test_client.py",
                _ => "test_shipping.py",
            };
            run_test(workspace, test).await?;
            anyhow::ensure!(
                final_text.contains(&task.sentinel),
                "final task sentinel missing"
            );
            if task.family == Family::MultiFileChange {
                for file in ["config.py", "client.py", "README.md"] {
                    let text = std::fs::read_to_string(workspace.join(file))?;
                    anyhow::ensure!(
                        !text.contains("MAX_RETRIES") && text.contains("MAX_ATTEMPTS"),
                        "multi-file rename incomplete: {file}"
                    );
                }
            }
        }
        Family::ToolSkillDiscovery => {
            let label = std::fs::read_to_string(workspace.join("label.txt"))?;
            anyhow::ensure!(
                label.trim() == format!("P-{} ROUTE-{}", task.sentinel, task.sentinel),
                "discovered label artifact mismatch"
            );
        }
        Family::RelevantMemoryRecall => anyhow::ensure!(
            final_text.contains(&format!("MEM-{}", task.sentinel))
                && final_text.contains("Bangkok"),
            "cold-session memory recall failed"
        ),
        Family::InterruptedExecutionRecovery => {
            let report = std::fs::read_to_string(workspace.join("recovery-report.txt"))?;
            anyhow::ensure!(
                report.contains(&format!("REC-{}", task.sentinel)),
                "recovered task report missing"
            );
        }
    }
    Ok(())
}

fn reject_extra_files(
    root: &Path,
    directory: &Path,
    task: &PairedTask,
    mutable: &[&str],
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        let kind = entry.file_type()?;
        anyhow::ensure!(
            !kind.is_symlink(),
            "symlink in paired workspace: {relative}"
        );
        if kind.is_dir() {
            if [".git", ".wayland-core", "__pycache__"]
                .contains(&entry.file_name().to_string_lossy().as_ref())
            {
                continue;
            }
            reject_extra_files(root, &path, task, mutable)?;
        } else if kind.is_file() {
            anyhow::ensure!(
                task.files.contains_key(&relative) || mutable.contains(&relative.as_str()),
                "unexpected task artifact: {relative}"
            );
        }
    }
    Ok(())
}

async fn run_test(workspace: &Path, test: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    let _guard = crate::process_tree::serialize_candidate_identity().await;
    let mut tree = ProcessTree::prepare()?;
    anyhow::ensure!(
        tree.is_authoritative(),
        "artifact execution requires authoritative process containment"
    );
    tree.prepare_workspace(workspace)?;
    let search_path = std::env::var_os("PATH").unwrap_or_default();
    let python = std::env::split_paths(&search_path)
        .map(|root| root.join("python3"))
        .find(|path| path.is_file())
        .ok_or_else(|| anyhow::anyhow!("python3 is required for artifact tests"))?
        .canonicalize()?;
    let executable = tree.prepare_executable(&python)?;
    let program = executable
        .path()
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Python executable path is not UTF-8"))?;
    let mut command = wcore_config::shell::shell_command_argv(program, &["-S", "-B", test]);
    command
        .current_dir(workspace)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    tree.configure(&mut command, Some(&executable))?;
    let mut child = command.spawn()?;
    drop(executable);
    tree.bind(&mut child).await?;
    let mut stdout = child.stdout.take().expect("piped stdout").take(65536);
    let mut stderr = child.stderr.take().expect("piped stderr").take(65536);
    let output = tokio::spawn(async move {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (a, b) = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        a?;
        b?;
        Ok::<_, std::io::Error>((out, err))
    });
    let status = tree
        .wait_for_exit_and_cleanup(&mut child, Duration::from_secs(15))
        .await?;
    let Some((status, cleanup)) = status else {
        tree.terminate(&mut child).await?;
        let _ = output.await;
        anyhow::bail!("paired artifact test timed out");
    };
    if let Some(error) = cleanup {
        return Err(error.into());
    }
    let (out, err) = output.await??;
    anyhow::ensure!(
        status.success(),
        "artifact test {test} failed: {} {}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&err)
    );
    Ok(())
}
