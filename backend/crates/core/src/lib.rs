//! Business objects, the SQLite schema and the command layer (notes/data-model.md).
//!
//! Commands and reads use an explicitly supplied database. Project records, Angela and worker
//! tasks share the instance schema; cross-project host state has its own database (`host`).

pub mod command;
pub mod db;
pub mod error;
pub mod host;
pub mod id;
pub mod item;
pub mod ledger;
pub mod manager;
pub mod project;
mod prompts;
pub mod question;
pub mod quota;
pub mod report;
pub mod role;
mod sql;
pub mod task;
pub mod turn;
pub mod view;
pub mod worker;

pub use command::{Caller, Command, Cx};
pub use db::Db;
pub use error::{Error, Result};
