//! The agent driver: runs a role's turns on a harness CLI and keeps their transcripts
//! (harness-adapter.md; data-model.md §3, §7). The backend drives Angela with it, and the worker
//! host drives its tasks' executors (tech-plan.md §7, worker-interface.md §11).
//!
//! What differs between the two is behind [`AgentHost`]: where a role's CLI runs, its
//! instructions, its permission mode, and what a quota rejection blocks.

pub mod capability;
pub mod harness;
pub mod instance;
pub mod launch;
pub mod mcp;
pub mod runner;

use std::path::PathBuf;

use lobotomy_core::role::Role;
use lobotomy_core::turn::{Failure, Turn};
use lobotomy_harness::{Harness, Permission};

pub use crate::harness::{Cli, HarnessConfig};
pub use crate::instance::Instance;

/// What the process that owns an instance provides to the driver: the backend's host for Angela
/// and the projects, the worker host for its tasks.
pub trait AgentHost: Send + Sync + Sized + 'static {
    /// How to start the harness CLIs.
    fn harness(&self) -> &HarnessConfig;

    /// The harness's permission mode (harness-adapter.md §1.9). Each turn reads it, so a change
    /// applies from the next turn on.
    fn permission(&self, harness: Harness) -> anyhow::Result<Permission>;

    /// Whether the harness's quota domain is blocked: no new turn starts there
    /// (data-model.md §8.3).
    fn is_blocked(&self, harness: &str) -> anyhow::Result<bool>;

    /// A turn was refused for its quota. Called before the turn ends: whoever sees the ended
    /// turn, the scheduler included, sees the block (data-model.md §8.3).
    fn quota_rejected(&self, harness: &str, failure: &Failure) -> anyhow::Result<()>;

    /// The MCP service's base URL, such as `http://127.0.0.1:4100`.
    fn mcp_base(&self) -> String;

    /// The role's instructions (roles-and-tasks.md §1.1).
    fn instructions(&self, role: &Role) -> String;

    /// Where the turn's CLI runs, created if missing.
    fn working_dir(&self, instance: &Instance<Self>, turn: &Turn, role: &Role) -> anyhow::Result<PathBuf>;

    /// Variables the turn's CLI gets besides the trimmed environment (`capability`), such as its
    /// own temporary directory.
    fn env(&self, _instance: &Instance<Self>, _turn: &Turn) -> anyhow::Result<Vec<(String, String)>> {
        Ok(Vec::new())
    }
}

/// A rejected runtime command means its preconditions do not hold now. Anything else, a broken
/// invariant included, is an error: waiting does not resolve it (#16).
pub fn ignore_rejection<T>(result: anyhow::Result<T>) -> anyhow::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) => match e.downcast_ref::<lobotomy_core::Error>().and_then(|e| e.code()) {
            Some(code) => {
                tracing::debug!(code, "not now: {e:#}");
                Ok(None)
            }
            None => Err(e),
        },
    }
}
