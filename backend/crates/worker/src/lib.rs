//! The worker host that comes with Lobotomy (worker-interface.md): it takes tasks from the
//! orchestrator over the worker protocol and runs each with the agent driver, in a clone of the
//! repository it is given (`environment`), or else in a plain folder of the task's own.

pub mod environment;
pub mod forge;
pub mod host;
pub mod server;
pub mod store;
pub mod task;

use std::sync::Arc;

use lobotomy_agent::mcp::{OrgTools, ToolTarget, router};
use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use crate::host::WorkerHost;
use crate::task::Tasks;

/// Runs the worker host: its agents' MCP service on a loopback port, its tasks, and the protocol
/// on `input` and `output` until the orchestrator closes `input`. Then the running CLIs end with
/// their process trees, without an interrupt first (harness-adapter.md §1.8).
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
    tasks.shutdown().await;
    serving.cancel();
    let _ = server.await;
    served
}
