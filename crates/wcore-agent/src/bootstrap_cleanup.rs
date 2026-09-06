//! Cleanup authority retained by a host across successful or failed bootstrap.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;
use wcore_mcp::manager::McpManager;
use wcore_plugin_subprocess::{McpBridgePluginRunner, SubprocessPluginRunner};

#[derive(Clone)]
enum Process {
    Mcp(Arc<McpManager>),
    Sdk(Arc<SubprocessPluginRunner>),
    Bridge(Arc<McpBridgePluginRunner>),
}
type Task = Arc<AsyncMutex<Option<JoinHandle<()>>>>;

#[derive(Default)]
pub struct BootstrapCleanup {
    started: AtomicBool,
    processes: Mutex<Vec<Process>>,
    tasks: Mutex<Vec<Task>>,
    closing: AsyncMutex<()>,
}

impl BootstrapCleanup {
    pub(crate) fn begin(&self) {
        self.started.store(true, Ordering::Release);
    }
    pub(crate) fn mcp(&self, manager: Arc<McpManager>) {
        self.processes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Process::Mcp(manager));
    }
    pub(crate) fn sdk(&self, plugin: Arc<SubprocessPluginRunner>) {
        self.processes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Process::Sdk(plugin));
    }
    pub(crate) fn bridge(&self, runner: Arc<McpBridgePluginRunner>) {
        self.processes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Process::Bridge(runner));
    }
    pub(crate) fn retain_tasks(&self, tasks: impl IntoIterator<Item = JoinHandle<()>>) {
        let mut saved = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        for task in tasks {
            task.abort();
            saved.push(Arc::new(AsyncMutex::new(Some(task))));
        }
    }

    /// Call only after the bootstrap/turn owner has joined and can no longer
    /// register resources. Failed attempts preserve handles for another close.
    pub async fn close(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.started.load(Ordering::Acquire),
            "bootstrap cleanup ownership was never established"
        );
        let _close = self.closing.lock().await;
        let processes = self
            .processes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let processes =
            futures::future::join_all(processes.into_iter().map(|process| async move {
                match process {
                    Process::Mcp(manager) => {
                        let results = futures::future::join_all(
                            manager.cleanup_server_names().into_iter().map(|name| {
                                let manager = manager.clone();
                                async move { manager.close_server(&name).await }
                            }),
                        )
                        .await;
                        let errors: Vec<_> = results
                            .into_iter()
                            .filter_map(Result::err)
                            .map(|e| e.to_string())
                            .collect();
                        anyhow::ensure!(errors.is_empty(), "MCP cleanup: {}", errors.join("; "));
                        Ok(())
                    }
                    Process::Sdk(plugin) => plugin.shutdown().await.map_err(anyhow::Error::from),
                    Process::Bridge(runner) => runner.shutdown().await.map_err(anyhow::Error::from),
                }
            }));
        let tasks = futures::future::join_all(tasks.into_iter().map(|task| async move {
            let mut saved = task.lock().await;
            let result = match saved.as_mut() {
                Some(task) => task.await,
                None => Ok(()),
            };
            saved.take();
            match result {
                Ok(()) => Ok(()),
                Err(error) if error.is_cancelled() => Ok(()),
                Err(error) => Err(anyhow::Error::from(error)),
            }
        }));
        let (processes, tasks) = tokio::join!(processes, tasks);
        let errors: Vec<_> = processes
            .into_iter()
            .chain(tasks)
            .filter_map(Result::err)
            .map(|error| error.to_string())
            .collect();
        anyhow::ensure!(
            errors.is_empty(),
            "bootstrap cleanup incomplete: {}",
            errors.join("; ")
        );
        self.processes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.tasks.lock().unwrap_or_else(|e| e.into_inner()).clear();
        Ok(())
    }
}

impl wcore_plugin_subprocess::RuntimeCleanupOwner for BootstrapCleanup {
    fn sdk_started(&self, runner: Arc<SubprocessPluginRunner>) {
        self.sdk(runner);
    }
    fn mcp_bridge_started(&self, runner: Arc<McpBridgePluginRunner>) {
        self.bridge(runner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct HeldDrop {
        entered: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        release: std::sync::mpsc::Receiver<()>,
    }
    impl Drop for HeldDrop {
        fn drop(&mut self) {
            self.entered.store(true, Ordering::Release);
            let _ = self.release.recv_timeout(Duration::from_secs(5));
            self.finished.store(true, Ordering::Release);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cleanup_retry_waits_for_owned_task_resource_drop() {
        let cleanup = BootstrapCleanup::default();
        cleanup.begin();
        let entered = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let (release, gate) = std::sync::mpsc::channel();
        let (started, ready) = tokio::sync::oneshot::channel();
        let held = HeldDrop {
            entered: entered.clone(),
            finished: finished.clone(),
            release: gate,
        };
        let task = tokio::spawn(async move {
            let _resource = held;
            let _ = started.send(());
            std::future::pending::<()>().await;
        });
        ready.await.unwrap();
        cleanup.retain_tasks([task]);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !entered.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("resource drop must start");
        let first = tokio::time::timeout(Duration::from_millis(20), cleanup.close()).await;
        let finished_before_release = finished.load(Ordering::Acquire);
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), cleanup.close())
            .await
            .expect("bounded cleanup retry")
            .expect("joined cleanup");
        assert!(
            first.is_err(),
            "cleanup acknowledged before its resource was released"
        );
        assert!(
            !finished_before_release,
            "positive control did not hold the resource"
        );
        assert!(finished.load(Ordering::Acquire));
        assert!(cleanup.tasks.lock().unwrap().is_empty());
    }
}
