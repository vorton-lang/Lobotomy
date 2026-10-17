//! The worker host that comes with Lobotomy (worker-interface.md): it takes tasks from the
//! orchestrator over the worker protocol and runs each with the agent driver. Until the
//! environment has a repository (tech-plan.md §7 step 1, group 5), each task works in a plain
//! folder of its own.

pub mod host;
pub mod server;
pub mod store;
pub mod task;

use std::sync::Arc;
use std::time::{Duration, Instant};

use lobotomy_agent::mcp::{OrgTools, ToolTarget, router};
use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use crate::host::WorkerHost;
use crate::task::Tasks;

/// How long quitting waits for interrupted CLIs to exit.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Runs the worker host: its agents' MCP service on a loopback port, its tasks, and the protocol
/// on `input` and `output` until the orchestrator closes `input`. Then it interrupts the running
/// CLIs and waits for them a while; those left end with their jobs when the process exits.
pub async fn run(
    host: Arc<WorkerHost>,
    input: impl AsyncBufRead + Unpin,
    output: impl AsyncWrite + Unpin + Send + 'static,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    *host.mcp_base.lock().unwrap() = format!("http://{}", listener.local_addr()?);
    let tasks = Tasks::new(host.clone());
    let serving = CancellationToken::new();
    let app = {
        let tasks = tasks.clone();
        router(OrgTools::new, move |id| tasks.instance(id).map(|i| ToolTarget::of(&i)), serving.clone())
    };
    let stop = serving.clone();
    let server = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(stop.cancelled_owned()).await {
            tracing::error!(error = %e, "the MCP server stopped");
        }
    });
    tasks.recover().await?;
    let served = server::serve(tasks.clone(), input, output).await;
    tasks.shutdown(Instant::now() + SHUTDOWN_GRACE).await;
    serving.cancel();
    let _ = server.await;
    served
}
