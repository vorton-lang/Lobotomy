//! The backend's MCP routes (harness-adapter.md §2): the executor's tools at
//! `/mcp/{project}/{token}`, Angela's at `/mcp/angela/{token}` (`angela`).

pub mod angela;

use std::sync::Arc;

use axum::Router;
use lobotomy_agent::mcp::{OrgTools, ToolTarget, router};
use tokio_util::sync::CancellationToken;

use crate::projects::Projects;

/// The MCP routes with the tools roles call, for every open project.
pub fn org_router(projects: Arc<Projects>, shutdown: CancellationToken) -> Router {
    router(OrgTools::new, move |id| projects.get(id).map(|p| ToolTarget::of(&p)), shutdown)
}
