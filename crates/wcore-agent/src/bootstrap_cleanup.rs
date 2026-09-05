//! Cleanup authority retained by a host across successful or failed bootstrap.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;
use wcore_mcp::manager::McpManager;
use wcore_plugin_subprocess::{LoadedSubprocessPlugin, McpBridgePluginRunner};

#[derive(Clone)]
enum Process {
    Mcp(Arc<McpManager>),
    Sdk(Arc<LoadedSubprocessPlugin>),
    Bridge(Arc<McpBridgePluginRunner>),
}
type Task = Arc<AsyncMutex<Option<JoinHandle<()>>>>;

#[derive(Default)]
pub struct BootstrapCleanup {
    started: AtomicBool,
    processes: Mutex<Vec<Process>>,
    tasks: Mutex<Vec<Task>>,
    watchers: Mutex<Vec<Arc<AsyncMutex<wcore_skills::watcher::SkillWatcher>>>>,
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
    pub(crate) fn sdk(&self, plugin: Arc<LoadedSubprocessPlugin>) {
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
    pub(crate) fn watcher(&self, watcher: wcore_skills::watcher::SkillWatcher) {
        self.watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::new(AsyncMutex::new(watcher)));
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
        let watchers = self
            .watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let processes =
            futures::future::join_all(processes.into_iter().map(|process| async move {
                match process {
                    Process::Mcp(manager) => {
                        let results = futures::future::join_all(
                            manager.server_names().into_iter().map(|name| {
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
                    Process::Sdk(plugin) => {
                        plugin.runner.shutdown().await.map_err(anyhow::Error::from)
                    }
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
        let watchers = futures::future::join_all(watchers.into_iter().map(|watcher| async move {
            watcher
                .lock()
                .await
                .stop_and_join()
                .await
                .map_err(anyhow::Error::from)
        }));
        let (processes, tasks, watchers) = tokio::join!(processes, tasks, watchers);
        let errors: Vec<_> = processes
            .into_iter()
            .chain(tasks)
            .chain(watchers)
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
        self.watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        Ok(())
    }
}
