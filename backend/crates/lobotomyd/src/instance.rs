//! The backend's project and Angela instances (data-model.md §10.6), using the agent driver's
//! storage on this backend's [`Host`]. Only Angela runs turns here; executors run in worker hosts.

pub use lobotomy_agent::instance::{LiveItem, LiveTurn, lock_file};

use crate::host::Host;

pub type Instance = lobotomy_agent::Instance<Host>;
