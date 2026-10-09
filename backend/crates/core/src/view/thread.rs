//! A role's thread, page by page, and search over it (frontend.md §4.1, §4.2; data-model.md §7).

use std::collections::{BTreeSet, HashMap};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::task::{Message, queued_messages, turn_messages};
use crate::turn::{Turn, load_turn};

/// Thread items per page.
const PAGE: i64 = 100;

#[derive(Deserialize)]
pub struct ThreadQuery {
    pub role: String,
    /// Items before this thread sequence number, for scrolling back.
    pub before: Option<i64>,
    /// Items after this sequence number, oldest first, to catch up. Without `before` or `after`,
    /// the newest page.
    pub after: Option<i64>,
    pub limit: Option<i64>,
}

/// A transcript item as stored (data-model.md §7.2). `content` holds the kind's fields
/// (harness/src/event.rs); a large field is a reference to the blob store (§7.3).
#[derive(Serialize)]
pub struct Item {
    pub id: String,
    pub seq: i64,
    pub turn_id: String,
    pub kind: String,
    pub content: Value,
    /// For a call to a Lobotomy tool: the command it ran.
    pub command_id: Option<String>,
    pub created_at: i64,
}

/// What a command was asked to do and what it returned.
#[derive(Serialize)]
pub struct CommandRecord {
    pub name: String,
    pub args: Value,
    pub result: Value,
}

/// A page of the thread with what its items refer to.
#[derive(Serialize)]
pub struct ThreadPage {
    pub items: Vec<Item>,
    /// Older items exist.
    pub has_more: bool,
    /// The turns of the page's items, by id.
    pub turns: HashMap<String, Turn>,
    /// The messages those turns delivered.
    pub messages: Vec<Message>,
    /// The command records of the page's Lobotomy tool calls, by id.
    pub commands: HashMap<String, CommandRecord>,
    /// Messages still waiting for a turn; they belong at the end of the newest page.
    pub queued: Vec<Message>,
}

pub fn thread_page(conn: &Connection, q: &ThreadQuery) -> Result<ThreadPage> {
    let limit = q.limit.unwrap_or(PAGE).clamp(1, 500);
    let order = if q.after.is_some() { "ASC" } else { "DESC" };
    let mut stmt = conn.prepare(&format!(
        "SELECT i.id, i.seq, i.turn_id, i.kind, i.content, i.command_id, i.created_at
         FROM item i JOIN thread t ON t.id = i.thread_id
         WHERE t.role = ?1 AND (?2 IS NULL OR i.seq < ?2) AND (?3 IS NULL OR i.seq > ?3)
         ORDER BY i.seq {order} LIMIT ?4"
    ))?;
    let rows = stmt.query_map(params![q.role, q.before, q.after, limit], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, String>(4)?, r.get(5)?, r.get(6)?))
    })?;
    let mut items = Vec::new();
    for row in rows {
        let (id, seq, turn_id, kind, content, command_id, created_at) = row?;
        items.push(Item { id, seq, turn_id, kind, content: serde_json::from_str(&content)?, command_id, created_at });
    }
    if q.after.is_none() {
        items.reverse();
    }
    let has_more = match items.first() {
        Some(first) => conn
            .query_row(
                "SELECT 1 FROM item i JOIN thread t ON t.id = i.thread_id WHERE t.role = ?1 AND i.seq < ?2 LIMIT 1",
                params![q.role, first.seq],
                |_| Ok(()),
            )
            .optional()?
            .is_some(),
        None => false,
    };

    let turn_ids: BTreeSet<&str> = items.iter().map(|i| i.turn_id.as_str()).collect();
    let mut turns = HashMap::new();
    let mut messages = Vec::new();
    for turn_id in turn_ids {
        turns.insert(turn_id.to_owned(), load_turn(conn, turn_id)?);
        messages.extend(turn_messages(conn, turn_id)?);
    }
    let mut commands = HashMap::new();
    for id in items.iter().filter_map(|i| i.command_id.as_deref()) {
        if let Some(record) = command_record(conn, id)? {
            commands.insert(id.to_owned(), record);
        }
    }
    let queued = queued_messages(conn, &q.role)?;
    Ok(ThreadPage { items, has_more, turns, messages, commands, queued })
}

fn command_record(conn: &Connection, id: &str) -> Result<Option<CommandRecord>> {
    let row: Option<(String, String, String)> = conn
        .query_row("SELECT name, args, result FROM command_record WHERE id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    row.map(|(name, args, result)| {
        Ok(CommandRecord { name, args: serde_json::from_str(&args)?, result: serde_json::from_str(&result)? })
    })
    .transpose()
}

// ---- search ----

#[derive(Deserialize)]
pub struct SearchQuery {
    pub role: String,
    pub query: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    Item,
    Message,
}

/// Where a match is: an item, or a message shown at the start of its turn (`seq` is the turn's
/// first item) or among the queued ones at the end (`seq` is absent).
#[derive(Serialize)]
pub struct Match {
    pub kind: MatchKind,
    pub id: String,
    pub seq: Option<i64>,
}

#[derive(Serialize)]
pub struct Found {
    pub matches: Vec<Match>,
    /// Older matches were left out.
    pub more: bool,
}

/// Matches returned at most, the newest; the GUI says when older ones were left out.
const MATCH_LIMIT: usize = 1000;

/// A text field of an item as the thread shows it: a field moved to the blob store is searched
/// by its head and tail, which is what the thread shows of it.
fn shown(field: &str) -> String {
    format!(
        "CASE json_type(i.content, '{field}') WHEN 'text' THEN json_extract(i.content, '{field}') \
         WHEN 'object' THEN coalesce(json_extract(i.content, '{field}.head'), '') || char(10) || \
         coalesce(json_extract(i.content, '{field}.tail'), '') END"
    )
}

/// The thread's matches of the query in order, ignoring ASCII case (frontend.md §4.1 "搜索"): the
/// text of items, the report a Lobotomy tool call recorded, and messages. The GUI loads pages
/// back to a match before showing it.
pub fn search(conn: &Connection, q: &SearchQuery) -> Result<Found> {
    let query = q.query.trim();
    if query.is_empty() {
        return Ok(Found { matches: vec![], more: false });
    }
    let text = ["$.text", "$.command", "$.output", "$.query", "$.message"]
        .iter()
        .map(|f| format!("coalesce({}, '')", shown(f)))
        .chain(["title", "body", "blocked_on"].iter().map(|k| format!("coalesce(json_extract(c.args, '$.{k}'), '')")))
        .collect::<Vec<_>>()
        .join(" || char(10) || ");
    let mut stmt = conn.prepare(&format!(
        "SELECT i.id, i.seq FROM item i JOIN thread t ON t.id = i.thread_id
         LEFT JOIN command_record c ON c.id = i.command_id
         WHERE t.role = ?1 AND i.kind != 'input' AND instr(lower({text}), lower(?2)) > 0
         ORDER BY i.seq DESC LIMIT ?3"
    ))?;
    let limit = MATCH_LIMIT as i64 + 1;
    let items = stmt.query_map(params![q.role, query, limit], |r| {
        Ok(Match { kind: MatchKind::Item, id: r.get(0)?, seq: Some(r.get(1)?) })
    })?;
    let mut matches = items.collect::<rusqlite::Result<Vec<_>>>()?;
    // A message bound to a turn that never stored an item is not shown, so it does not match.
    let mut stmt = conn.prepare(
        "SELECT m.id, m.turn_id, (SELECT MIN(i.seq) FROM item i WHERE i.turn_id = m.turn_id) FROM message m
         WHERE m.role = ?1 AND instr(lower(m.body), lower(?2)) > 0 ORDER BY m.seq",
    )?;
    let messages = stmt.query_map(params![q.role, query], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?))
    })?;
    for row in messages {
        let (id, turn, seq) = row?;
        if turn.is_none() || seq.is_some() {
            matches.push(Match { kind: MatchKind::Message, id, seq });
        }
    }
    // The thread's order: a turn's messages come before its items; queued messages come last.
    matches.sort_by_key(|m| (m.seq.unwrap_or(i64::MAX), m.kind == MatchKind::Item));
    let more = matches.len() > MATCH_LIMIT;
    if more {
        matches.drain(..matches.len() - MATCH_LIMIT);
    }
    Ok(Found { matches, more })
}
