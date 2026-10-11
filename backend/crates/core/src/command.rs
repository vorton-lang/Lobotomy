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
    /// Angela, the user's manager, in a project (data-model.md §10.6). Her turn is in her own
    /// database, not this one: the host's MCP entry checks there that it is unfinished before it
    /// runs her command here.
    Manager { turn_id: String },
}

impl Caller {
    pub fn scope(&self) -> String {
        match self {
            Caller::User => "user".into(),
            Caller::Runtime => "runtime".into(),
            Caller::Role { role, .. } => format!("role:{role}"),
            Caller::Manager { .. } => "manager".into(),
        }
    }

    /// The turn a role's or the manager's command came from.
    pub fn turn_id(&self) -> Option<&str> {
        match self {
            Caller::Role { turn_id, .. } | Caller::Manager { turn_id } => Some(turn_id),
            _ => None,
        }
    }

    pub fn require_user(&self) -> Result<()> {
        match self {
            Caller::User => Ok(()),
            other => Err(forbidden(other)),
        }
    }

    /// The control commands the manager runs for the user (data-model.md §10.6 跨库操作第 2 条).
    /// Without her, the user runs them all (roles-and-tasks.md §1.4).
    pub fn require_user_or_manager(&self) -> Result<()> {
        match self {
            Caller::User | Caller::Manager { .. } => Ok(()),
            other => Err(forbidden(other)),
        }
    }

    pub fn require_manager(&self) -> Result<()> {
        match self {
            Caller::Manager { .. } => Ok(()),
            other => Err(forbidden(other)),
        }
    }

    pub fn require_runtime(&self) -> Result<()> {
        match self {
            Caller::Runtime => Ok(()),
            other => Err(forbidden(other)),
        }
    }
}

fn forbidden(caller: &Caller) -> Error {
    Error::rejected("forbidden", format!("{} may not run this command", caller.scope()))
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
