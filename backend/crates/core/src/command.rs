use rusqlite::{Transaction, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Who issued a command. Idempotency keys are scoped per caller (data-model.md §9.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Caller {
    /// The user, through the GUI. Without Angela the user can run every control command.
    User,
    /// The runtime itself: scheduler, adapters, outbox executors.
    Runtime,
    /// A role calling through MCP, identified by the token of the turn it runs in.
    Role { role: String, turn_id: String },
}

impl Caller {
    pub fn scope(&self) -> String {
        match self {
            Caller::User => "user".into(),
            Caller::Runtime => "runtime".into(),
            Caller::Role { role, .. } => format!("role:{role}"),
        }
    }

    /// The turn a role command came from.
    pub fn turn_id(&self) -> Option<&str> {
        match self {
            Caller::Role { turn_id, .. } => Some(turn_id),
            _ => None,
        }
    }

    pub fn require_user(&self) -> Result<()> {
        match self {
            Caller::User => Ok(()),
            other => Err(Error::rejected("forbidden", format!("{} may not run this command", other.scope()))),
        }
    }

    pub fn require_runtime(&self) -> Result<()> {
        match self {
            Caller::Runtime => Ok(()),
            other => Err(Error::rejected("forbidden", format!("{} may not run this command", other.scope()))),
        }
    }
}

/// A domain command (data-model.md §1).
///
/// `apply` runs inside the transaction that `Db::execute` holds. It checks the caller and the
/// preconditions, writes business objects and emits events. An error rolls everything back.
pub trait Command: Serialize {
    const NAME: &'static str;
    type Output: Serialize + DeserializeOwned;

    /// Repeating a command with the same key returns the first result without running it again.
    fn idem_key(&self) -> Option<&str> {
        None
    }

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<Self::Output>;
}

/// The context a command runs in: the open transaction, the command's timestamp and the id its
/// command record will have.
pub struct Cx<'a> {
    pub tx: &'a Transaction<'a>,
    pub now: i64,
    command_id: &'a str,
}

impl<'a> Cx<'a> {
    pub(crate) fn new(tx: &'a Transaction<'a>, now: i64, command_id: &'a str) -> Self {
        Self { tx, now, command_id }
    }

    /// Appends to the global event log in the same transaction and returns the sequence number.
    /// The event names the command, so a reader can tell who caused it (data-model.md §10.6).
    pub fn emit(&mut self, kind: &str, entity: &str, payload: serde_json::Value) -> Result<i64> {
        self.tx.execute(
            "INSERT INTO event (kind, entity, payload, created_at, command_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![kind, entity, payload.to_string(), self.now, self.command_id],
        )?;
        Ok(self.tx.last_insert_rowid())
    }
}
