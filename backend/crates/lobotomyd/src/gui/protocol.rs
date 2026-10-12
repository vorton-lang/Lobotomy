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

// A route owns the wire name and the types used by both serde and TypeScript export.
pub(super) struct Route<P, R> {
    pub name: &'static str,
    types: std::marker::PhantomData<fn(P) -> R>,
}

impl<P, R> Route<P, R> {
    pub const fn new(name: &'static str) -> Self {
        Self { name, types: std::marker::PhantomData }
    }

    pub fn decode(&self, value: serde_json::Value) -> serde_json::Result<P>
    where
        P: serde::de::DeserializeOwned,
    {
        serde_json::from_value(value)
    }

    pub fn encode(&self, value: R) -> anyhow::Result<serde_json::Value>
    where
        R: Serialize,
    {
        Ok(serde_json::to_value(value)?)
    }
}

macro_rules! routes {
    ($register:ident, $kind:ident; $($vis:vis $id:ident: $params:ty => $result:ty = $name:literal;)*) => {
        $($vis const $id: super::protocol::Route<$params, $result> = super::protocol::Route::new($name);)*
        #[cfg(feature = "ts-rs")]
        pub(super) fn $register(out: &mut super::bindings::Bindings) {
            $(out.$kind::<$params, $result>($id.name);)*
        }
    };
}
pub(super) use routes;
