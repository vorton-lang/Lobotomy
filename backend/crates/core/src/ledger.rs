//! The manager's ledger (manager-actions.md §3): her judgments, kept apart from the runtime's facts.
//! Entries are only added; a change is a new entry that supersedes the old one. A decision and a
//! constraint rest on the user's words, which the runtime copies in with their id; without them a
//! judgment is a hypothesis. Each entry is in the database of its scope: the user's in hers, a
//! project's and a task's in the project's (data-model.md §10.6 "记录按作用域存放").

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::command::{Caller, Command, Cx};
use crate::error::{Error, Result};
use crate::id::new_id;
use crate::sql::sql_enum;
use crate::task::{Quote, load_task};

sql_enum! {
    pub enum LedgerKind {
        Goal = "goal",
        Phase = "phase",
        Decision = "decision",
        Constraint = "constraint",
        OpenQuestion = "open_question",
        Hypothesis = "hypothesis",
    }
}

impl LedgerKind {
    /// A decision and a constraint must cite the user's own words (manager-actions.md §3.2).
    pub fn needs_the_users_words(self) -> bool {
        matches!(self, LedgerKind::Decision | LedgerKind::Constraint)
    }

    pub fn label(self) -> &'static str {
        match self {
            LedgerKind::Goal => "目标",
            LedgerKind::Phase => "阶段",
            LedgerKind::Decision => "决定",
            LedgerKind::Constraint => "约束",
            LedgerKind::OpenQuestion => "待定",
            LedgerKind::Hypothesis => "推测",
        }
    }
}

/// Adds an entry to the ledger of this database's scope. The manager adds it: as `manager` in a
/// project's database, as her own role in hers.
#[derive(Debug, Serialize, Deserialize)]
pub struct AddLedgerEntry {
    pub kind: LedgerKind,
    pub statement: String,
    /// The user's words it rests on, which the runtime looked up in her database.
    pub evidence: Vec<Quote>,
    /// For an entry of a task's scope.
    pub task_id: Option<String>,
    pub confidence: Option<String>,
    pub supersedes: Option<String>,
}

impl Command for AddLedgerEntry {
    const NAME: &'static str = "add_ledger_entry";
    type Output = String;

    fn apply(&self, caller: &Caller, cx: &mut Cx<'_>) -> Result<String> {
        let turn_id = match caller {
            Caller::Manager { turn_id } => turn_id,
            Caller::Role { role, turn_id } if crate::role::load_role(cx.tx, role)?.kind == "manager" => turn_id,
            other => return Err(Error::rejected("forbidden", format!("{} keeps no ledger", other.scope()))),
        };
        if self.statement.trim().is_empty() {
            return Err(Error::rejected("missing_field", "an entry needs a statement"));
        }
        if self.kind.needs_the_users_words() && self.evidence.is_empty() {
            return Err(Error::rejected(
                "needs_the_users_words",
                format!("a {} rests on the user's own words; without them it is a hypothesis", self.kind.as_str()),
            ));
        }
        if let Some(task) = &self.task_id {
            load_task(cx.tx, task)?;
        }
        if let Some(old) = &self.supersedes {
            let exists = cx.tx.query_row("SELECT 1 FROM ledger_entry WHERE id = ?1", [old], |_| Ok(())).optional()?;
            if exists.is_none() {
                return Err(Error::rejected("not_found", format!("no entry {old} in this scope")));
            }
            let newer: Option<String> =
                cx.tx.query_row("SELECT id FROM ledger_entry WHERE supersedes = ?1", [old], |r| r.get(0)).optional()?;
            if let Some(newer) = newer {
                return Err(Error::rejected("superseded", format!("{old} is superseded by {newer}; supersede that")));
            }
        }
        let id = new_id("led");
        cx.tx.execute(
            "INSERT INTO ledger_entry (id, kind, statement, task_id, confidence, supersedes, turn_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, self.kind, self.statement, self.task_id, self.confidence, self.supersedes, turn_id, cx.now],
        )?;
        for (pos, q) in self.evidence.iter().enumerate() {
            cx.tx.execute(
                "INSERT INTO ledger_evidence (entry_id, pos, source_id, said_at, body) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, pos as i64, q.source_id, q.said_at, q.body],
            )?;
        }
        cx.emit("ledger.added", &id, json!({ "kind": self.kind, "supersedes": self.supersedes }))?;
        Ok(id)
    }
}

/// An entry as it reads now: whether it still holds.
#[derive(Clone, Debug, Serialize)]
pub struct LedgerEntry {
    pub id: String,
    pub kind: LedgerKind,
    pub statement: String,
    pub task_id: Option<String>,
    pub task_title: Option<String>,
    pub confidence: Option<String>,
    pub supersedes: Option<String>,
    /// The entry that replaced it.
    pub superseded_by: Option<String>,
    /// A constraint of a task that has closed no longer holds (roles-and-tasks.md §2.2).
    pub lapsed: bool,
    pub evidence: Vec<Quote>,
    pub created_at: i64,
}

impl LedgerEntry {
    pub fn holds(&self) -> bool {
        self.superseded_by.is_none() && !self.lapsed
    }
}

/// The ledger of this database's scope, oldest first, each entry with whether it still holds.
pub fn ledger(conn: &Connection) -> Result<Vec<LedgerEntry>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, e.kind, e.statement, e.task_id, t.title, t.phase IN ('done', 'abandoned'), e.confidence,
                e.supersedes, (SELECT n.id FROM ledger_entry n WHERE n.supersedes = e.id), e.created_at
         FROM ledger_entry e LEFT JOIN task t ON t.id = e.task_id ORDER BY e.created_at, e.id",
    )?;
    let rows = stmt.query_map([], |r| {
        let kind: LedgerKind = r.get(1)?;
        let closed: Option<bool> = r.get(5)?;
        Ok(LedgerEntry {
            id: r.get(0)?,
            kind,
            statement: r.get(2)?,
            task_id: r.get(3)?,
            task_title: r.get(4)?,
            lapsed: kind == LedgerKind::Constraint && closed == Some(true),
            confidence: r.get(6)?,
            supersedes: r.get(7)?,
            superseded_by: r.get(8)?,
            evidence: Vec::new(),
            created_at: r.get(9)?,
        })
    })?;
    let mut entries = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt =
        conn.prepare("SELECT source_id, said_at, body FROM ledger_evidence WHERE entry_id = ?1 ORDER BY pos")?;
    for entry in &mut entries {
        let quotes =
            stmt.query_map([&entry.id], |r| Ok(Quote { source_id: r.get(0)?, said_at: r.get(1)?, body: r.get(2)? }))?;
        entry.evidence = quotes.collect::<rusqlite::Result<_>>()?;
    }
    Ok(entries)
}

/// The entries that hold, a line each, as the manager reads them when a new session of hers
/// starts (manager-actions.md §3.4). Hypotheses come last, apart.
pub fn ledger_lines(entries: &[LedgerEntry]) -> Vec<String> {
    let line = |e: &LedgerEntry| {
        let task = e.task_title.as_ref().map(|t| format!("（任务「{t}」）")).unwrap_or_default();
        let sure = e.confidence.as_ref().map(|c| format!("（把握：{c}）")).unwrap_or_default();
        let words: Vec<String> = e
            .evidence
            .iter()
            .map(|q| {
                let first: String = q.body.lines().next().unwrap_or_default().chars().take(60).collect();
                format!("「{first}」{}", q.source_id)
            })
            .collect();
        let because = if words.is_empty() { String::new() } else { format!("；用户原话：{}", words.join("、")) };
        format!("[{}] {}{task}{sure}{because}（{}）", e.kind.label(), e.statement, e.id)
    };
    let (guesses, rest): (Vec<&LedgerEntry>, Vec<&LedgerEntry>) =
        entries.iter().filter(|e| e.holds()).partition(|e| e.kind == LedgerKind::Hypothesis);
    rest.into_iter().chain(guesses).map(line).collect()
}
