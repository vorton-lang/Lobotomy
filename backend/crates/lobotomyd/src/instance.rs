//! The backend's instances: a project, or Angela (data-model.md §10.6), each run by the agent
//! driver on this backend's [`Host`].

pub use lobotomy_agent::instance::{LiveItem, LiveTurn, lock_file};

use crate::host::Host;

pub type Instance = lobotomy_agent::Instance<Host>;
