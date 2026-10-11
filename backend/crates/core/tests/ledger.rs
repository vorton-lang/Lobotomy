//! The manager's ledger (manager-actions.md §3): only she adds to it; a decision and a constraint
//! rest on the user's words; a change supersedes, once; a task's constraint lapses when it closes.

mod common;

use common::*;
use lobotomy_core::Caller;
use lobotomy_core::ledger::{AddLedgerEntry, LedgerKind, ledger, ledger_lines};
use lobotomy_core::task::{Abandon, Quote};

fn angela() -> Caller {
    Caller::Manager { turn_id: "turn_of_angela".into() }
}

fn entry(kind: LedgerKind, statement: &str, said: Option<&str>) -> AddLedgerEntry {
    AddLedgerEntry {
        kind,
        statement: statement.into(),
        evidence: said
            .map(|body| vec![Quote { source_id: "msg_user1".into(), said_at: 1_760_000_000_000, body: body.into() }])
            .unwrap_or_default(),
        task_id: None,
        confidence: None,
        supersedes: None,
    }
}

#[test]
fn a_decision_rests_on_the_users_words_and_only_she_keeps_the_ledger() {
    let db = db();
    let bare = entry(LedgerKind::Decision, "先做 CSV", None);
    assert_eq!(rejection(db.execute(&angela(), &bare)), "needs_the_users_words");
    let guess = AddLedgerEntry {
        confidence: Some("中".into()),
        ..entry(LedgerKind::Hypothesis, "用户更在意速度", None)
    };
    db.execute(&angela(), &guess).unwrap();
    let decision = entry(LedgerKind::Decision, "先做 CSV", Some("先做 CSV 吧"));
    assert_eq!(rejection(db.execute(&Caller::User, &decision)), "forbidden");
    let executor = Caller::Role { role: ROLE.into(), turn_id: "turn_x".into() };
    assert_eq!(rejection(db.execute(&executor, &decision)), "forbidden", "an executor keeps no ledger");
    let id = db.execute(&angela(), &decision).unwrap();
    let entries = db.read(ledger).unwrap();
    let kept = entries.iter().find(|e| e.id == id).unwrap();
    assert_eq!(kept.evidence[0].source_id, "msg_user1");
    let lines = ledger_lines(&entries);
    assert!(lines[0].starts_with("[决定] 先做 CSV；用户原话：「先做 CSV 吧」msg_user1"), "{lines:?}");
    assert!(lines[1].starts_with("[推测] 用户更在意速度（把握：中）"), "hypotheses come last: {lines:?}");
}

#[test]
fn a_change_supersedes_the_entry_once() {
    let db = db();
    let first = db.execute(&angela(), &entry(LedgerKind::Goal, "导出报表", None)).unwrap();
    let change =
        AddLedgerEntry { supersedes: Some(first.clone()), ..entry(LedgerKind::Goal, "导出报表和图表", None) };
    let second = db.execute(&angela(), &change).unwrap();
    assert_eq!(rejection(db.execute(&angela(), &change)), "superseded", "supersede the newest one");
    let missing = AddLedgerEntry { supersedes: Some("led_none".into()), ..entry(LedgerKind::Goal, "x", None) };
    assert_eq!(rejection(db.execute(&angela(), &missing)), "not_found");
    let entries = db.read(ledger).unwrap();
    let old = entries.iter().find(|e| e.id == first).unwrap();
    assert_eq!((old.superseded_by.as_deref(), old.holds()), (Some(second.as_str()), false));
    assert_eq!(ledger_lines(&entries).len(), 1);
}

/// roles-and-tasks.md §2.2: a constraint of a task's scope lapses when the task closes.
#[test]
fn a_tasks_constraint_lapses_when_the_task_closes() {
    let db = db();
    let task = create(&db, "c1");
    let constraint = AddLedgerEntry {
        task_id: Some(task.clone()),
        ..entry(LedgerKind::Constraint, "不改公共接口", Some("别动接口"))
    };
    db.execute(&angela(), &constraint).unwrap();
    assert!(db.read(ledger).unwrap()[0].holds());
    db.execute(&Caller::User, &Abandon { request_id: "a1".into(), task_id: task, reason: "不做了".into() }).unwrap();
    let entries = db.read(ledger).unwrap();
    assert!(entries[0].lapsed && ledger_lines(&entries).is_empty());
}
