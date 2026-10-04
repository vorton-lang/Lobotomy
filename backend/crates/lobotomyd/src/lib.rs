//! The Lobotomy backend: project instances and the services roles and the GUI call: MCP at
//! `/mcp/{token}`, the GUI at `/gui`.

pub mod capability;
pub mod gui;
pub mod host;
pub mod mcp;
pub mod onboard;
pub mod project;
pub mod results;
pub mod runner;
pub mod scheduler;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::project::Project;

/// A running backend for one project: the MCP service, the GUI service and the scheduler.
pub struct Backend {
    pub project: Arc<Project>,
    pub addr: SocketAddr,
    scheduling: CancellationToken,
    serving: CancellationToken,
    server: JoinHandle<()>,
    scheduler: JoinHandle<()>,
}

impl Backend {
    /// Listens on 127.0.0.1:`port` (0 picks a free port), reconciles turns left by the last run
    /// and starts scheduling.
    pub async fn start(project: Arc<Project>, port: u16) -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let addr = listener.local_addr()?;
        *project.mcp_base.lock().unwrap() = format!("http://{addr}");
        scheduler::recover(&project).await?;

        let serving = CancellationToken::new();
        let app = mcp::org_router(project.clone(), serving.clone()).merge(gui::gui_router(project.clone()));
        tokio::spawn(gui::watch(project.clone(), serving.clone()));
        let stop = serving.clone();
        let server = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(stop.cancelled_owned()).await {
                tracing::error!(error = %e, "the HTTP server stopped");
            }
        });
        let scheduling = CancellationToken::new();
        let scheduler = tokio::spawn(scheduler::run(project.clone(), scheduling.clone()));
        Ok(Self { project, addr, scheduling, serving, server, scheduler })
    }

    /// Normal shutdown (harness-adapter.md §1.8): stop scheduling, interrupt the running CLIs and
    /// wait for them to exit. The MCP service stays up meanwhile, so a CLI can still report. After
    /// `grace`, the remaining CLIs end with their jobs when the process exits.
    pub async fn shutdown(self, grace: Duration) {
        self.scheduling.cancel();
        let _ = self.scheduler.await;
        let running: Vec<String> = self.project.running.lock().unwrap().keys().cloned().collect();
        for turn_id in running {
            if let Err(e) = runner::interrupt(&self.project, &turn_id).await {
                tracing::warn!(turn_id, error = format!("{e:#}"), "could not interrupt");
            }
        }
        let deadline = Instant::now() + grace;
        while !self.project.running.lock().unwrap().is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        self.serving.cancel();
        let _ = self.server.await;
    }
}
