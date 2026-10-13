//! The Lobotomy backend: one process for all projects of a host directory and for Angela
//! (data-model.md §10), and the services roles and the GUI call: MCP at `/mcp/{project}/{token}`
//! and `/mcp/angela/{token}`, the GUI at `/gui`.

pub mod angela;
pub mod capability;
pub mod gui;
pub mod host;
pub mod instance;
pub mod launch;
pub mod mcp;
pub mod onboard;
pub mod project;
pub mod projects;
pub mod results;
pub mod runner;
pub mod scheduler;
pub mod trial;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::angela::Angela;
use crate::host::Host;
use crate::instance::Instance;
use crate::project::Project;
use crate::projects::Projects;

/// A running backend: the projects it opened, Angela, the MCP service and the GUI service.
pub struct Backend {
    pub host: Arc<Host>,
    pub projects: Arc<Projects>,
    /// Angela, or why she could not be opened: the projects run without her (roles-and-tasks.md
    /// §1.4).
    pub angela: Result<Angela, String>,
    pub addr: SocketAddr,
    serving: CancellationToken,
    server: JoinHandle<()>,
    gui_sessions: TaskTracker,
}

impl Backend {
    /// Listens on 127.0.0.1:`port` (0 picks a free port), opens the registered projects and
    /// Angela, and serves. A project that cannot be opened does not stop the others, nor Angela
    /// the projects (data-model.md §10.3).
    pub async fn start(host: Arc<Host>, port: u16) -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
        let addr = listener.local_addr()?;
        *host.mcp_base.lock().unwrap() = format!("http://{addr}");
        let projects = Arc::new(Projects::new(host.clone()));
        projects.open_registered().await?;
        let angela = Angela::start(host.clone(), projects.clone()).await.map_err(|e| {
            tracing::error!(error = format!("{e:#}"), "Angela could not be opened");
            format!("{e:#}")
        });
        let angela_instance = angela.as_ref().map(|a| a.instance.clone()).map_err(Clone::clone);

        let serving = CancellationToken::new();
        let gui_sessions = TaskTracker::new();
        let mut app = mcp::org_router(projects.clone(), serving.clone()).merge(gui::gui_router(
            host.clone(),
            projects.clone(),
            angela_instance.clone(),
            serving.clone(),
            gui_sessions.clone(),
        ));
        if let Ok(instance) = angela_instance {
            let reach = mcp::angela::Reach { angela: instance, projects: projects.clone() };
            app = app.merge(mcp::angela::router(reach, serving.clone()));
        }
        let stop = serving.clone();
        let server = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(stop.cancelled_owned()).await {
                tracing::error!(error = %e, "the HTTP server stopped");
            }
        });
        Ok(Self { host, projects, angela, addr, serving, server, gui_sessions })
    }

    /// An open project.
    pub fn project(&self, id: &str) -> Option<Arc<Project>> {
        self.projects.get(id)
    }

    /// Angela's instance, if she opened.
    pub fn angela(&self) -> Option<Arc<Instance>> {
        self.angela.as_ref().ok().map(|a| a.instance.clone())
    }

    /// Normal shutdown (harness-adapter.md §1.8): every project and Angela stop scheduling,
    /// interrupt their running CLIs and wait for them until `grace` has passed; the MCP service
    /// stays up meanwhile. After `grace`, the remaining CLIs end with their jobs when the process
    /// exits. GUI requests already admitted get what is left of the grace period.
    pub async fn shutdown(self, grace: Duration) {
        let deadline = Instant::now() + grace;
        let angela = async {
            if let Ok(angela) = self.angela {
                angela.stop(deadline).await;
            }
        };
        futures::future::join(self.projects.stop_all(deadline), angela).await;
        // A shutdown command must be able to flush its reply before cancellation drops the
        // socket writer. A slow client gets only the remaining, existing shutdown grace.
        self.host.shutdown_replies.close();
        let _ = tokio::time::timeout_at(deadline.into(), self.host.shutdown_replies.wait()).await;
        self.serving.cancel();
        let _ = self.server.await;
        // No new upgrades after the server stopped.
        self.gui_sessions.close();
        if tokio::time::timeout_at(deadline.into(), self.gui_sessions.wait()).await.is_err() {
            tracing::warn!("shutdown grace elapsed with GUI requests still running");
        }
    }
}
