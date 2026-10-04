//! The project's private store and the directories materialized from it
//! (harness-adapter.md §3, §4; data-model.md §5).
//!
//! jj holds immutable results: captures, candidates and integration versions are commits in a
//! git-backed jj store inside Lobotomy's data directory. Working directories are only execution
//! sites. jj's types stay inside this crate; callers see commit ids as hex strings.

mod eol;
mod error;
mod git;
pub mod repo;
mod store;
mod workspace;

pub use error::{Error, Result};
pub use store::{Composed, Content, FileChange, Identity, Store};
pub use workspace::{Captured, Guard, Leave, NewFile, Scope, Uncovered, Workspace};
