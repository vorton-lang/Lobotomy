//! Persisted changes pushed to the GUI. Live items still have a separate, dynamic contract.

use lobotomy_core::view::Event;
use serde::Serialize;

/// A project id of `angela` names the manager's database.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum ChangePush<'a> {
    Events { project: &'a str, events: &'a [Event] },
    Thread { project: &'a str, role: &'a str, seq: i64 },
}
