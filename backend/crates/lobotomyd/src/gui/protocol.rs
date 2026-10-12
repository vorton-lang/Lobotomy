//! Messages pushed to the GUI; live item content remains arbitrary JSON.

use lobotomy_core::view::Event;
use serde::Serialize;

/// A project id of `angela` names the manager's database.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub enum Push<'a> {
    Events { project: &'a str, events: &'a [Event] },
    Live { project: &'a str, live: &'a std::collections::HashMap<String, crate::instance::LiveTurn> },
    Host,
    Tick,
    Resync,
    Thread { project: &'a str, role: &'a str, seq: i64 },
}

#[derive(serde::Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct NoParams {}

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct RemoteError<'a> {
    pub code: &'a str,
    pub message: String,
}
