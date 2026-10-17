//! What wakes the manager and what she is told (manager-actions.md §6 "Manager 什么时候运行";
//! data-model.md §10.6 跨库操作第 4、5 条).

mod common;

use common::*;
use lobotomy_core::Caller;
use lobotomy_core::manager::{AskUser, Cursor, QuestionKind, ScheduleManagerTurn, cursors, open_questions};
use lobotomy_core::task::queued_messages;
use lobotomy_core::view::{Attention, attention, last_event, overview};

/// Her question waits in "等你决定" until a message the user sent after it is delivered to her;
/// one sent before it, still queued, answers nothing (manager-actions.md §6 `ask_user`). The role
/// stands in for her: her database has the projects' schema.
#[test]
fn her_question_waits_for_a_message_sent_after_it() {
    let db = db();
    send(&db, "m1", None, "看看现状");
    let t1 = register(&db);
    send(&db, "m2", None, "顺便问一下进度");
    let asker = Caller::Role { role: ROLE.into(), turn_id: t1.turn_id.clone() };
    let ask =
        AskUser { question: "导出先做 CSV 还是 Excel？".into(), kind: QuestionKind::Open, options: Vec::new() };
    let id = db.execute(&asker, &ask).unwrap();
    let open = || db.read(open_questions).unwrap();
    assert_eq!(open()[0].id, id);
    let overview = db.read(|c| overview(c, "lobotomy")).unwrap();
    let asked = attention(&overview, &[], |_| false);
    assert!(asked.iter().any(|a| matches!(a, Attention::Question { question, .. } if question.starts_with("导出"))));
    end_turn(&db, &t1.turn_id);
    assert_eq!(common::rejection(db.execute(&asker, &ask)), "turn_ended");

    let t2 = register(&db);
    assert_eq!(open().len(), 1, "m2 was sent before the question");
    end_turn(&db, &t2.turn_id);
    send(&db, "m3", None, "先做 CSV");
    register(&db);
    assert!(open().is_empty(), "the answer closes it");
}

fn prepared(db: &lobotomy_core::Db) -> ScheduleManagerTurn {
    ScheduleManagerTurn {
        role: ROLE.into(),
        as_of: db.read(last_event).unwrap(),
        messages: vec!["ledger for this session".into(), "project notice".into()],
        cursors: vec![Cursor { project_id: "prj_a".into(), seq: 9 }],
        register: true,
    }
}

#[test]
fn scheduling_commits_notices_cursors_and_turn_once() {
    let db = db();
    let said = send(&db, "m1", None, "user message");
    let command = prepared(&db);
    assert_eq!(rejection(db.execute(&Caller::User, &command)), "forbidden");
    let turn = db.execute(&Caller::Runtime, &command).unwrap().unwrap();
    let input = db.read(|c| lobotomy_core::turn::load_turn(c, &turn.turn_id)).unwrap().input;
    for text in ["user message", "ledger for this session", "project notice"] {
        assert!(input.contains(text), "{input}");
    }
    assert_eq!(db.read(cursors).unwrap(), command.cursors);
    let messages = db.read(|c| lobotomy_core::task::turn_messages(c, &turn.turn_id)).unwrap();
    assert_eq!(messages.iter().map(|m| m.source.as_str()).collect::<Vec<_>>(), ["user", "runtime", "runtime"]);
    assert!(db.read(|c| queued_messages(c, ROLE)).unwrap().is_empty());
    let after = db.read(last_event).unwrap();
    assert_eq!(rejection(db.execute(&Caller::Runtime, &command)), "manager_state_changed");
    assert_eq!(db.read(last_event).unwrap(), after, "replaying preparation duplicates no events");
    assert_eq!(send(&db, "m1", None, "user message"), said, "the user's request keeps its idempotency");
    assert_eq!(db.read(last_event).unwrap(), after);
}

#[test]
fn a_new_session_or_message_invalidates_preparation_without_consuming_it() {
    for session_changed in [false, true] {
        let db = db();
        send(&db, "m1", None, "waiting");
        let command = prepared(&db);
        if session_changed {
            db.execute(
                &Caller::User,
                &lobotomy_core::turn::NewNativeSession { request_id: "new".into(), role: ROLE.into() },
            )
            .unwrap();
        } else {
            send(&db, "m2", None, "arrived while preparing");
        }
        let after = db.read(last_event).unwrap();
        assert_eq!(rejection(db.execute(&Caller::Runtime, &command)), "manager_state_changed");
        assert_eq!(db.read(last_event).unwrap(), after);
        assert!(db.read(cursors).unwrap().is_empty());
        let inbox = db.read(|c| queued_messages(c, ROLE)).unwrap();
        assert_eq!(inbox.len(), if session_changed { 1 } else { 2 });
        assert!(inbox.iter().all(|m| m.source == "user"));
        assert!(db.read(|c| lobotomy_core::turn::unfinished_turn(c, ROLE)).unwrap().is_none());
    }
}

#[test]
fn a_registration_refusal_rolls_back_notices_and_cursors() {
    let db = db();
    send(&db, "m1", None, "already running");
    register(&db);
    let command = prepared(&db);
    assert_eq!(rejection(db.execute(&Caller::Runtime, &command)), "turn_unfinished");
    assert_eq!(db.read(last_event).unwrap(), command.as_of, "staged message events rolled back too");
    assert!(db.read(cursors).unwrap().is_empty());
    assert!(db.read(|c| queued_messages(c, ROLE)).unwrap().is_empty());
}

#[test]
fn an_idle_scheduler_read_only_initializes_cursors() {
    let db = db();
    let mut command = ScheduleManagerTurn { messages: vec![], register: false, ..prepared(&db) };
    assert!(db.execute(&Caller::Runtime, &command).unwrap().is_none());
    assert!(db.execute(&Caller::Runtime, &command).unwrap().is_none());
    assert_eq!(db.read(cursors).unwrap(), command.cursors);
    command.cursors[0].seq += 1;
    assert!(db.execute(&Caller::Runtime, &command).unwrap().is_none());
    assert_eq!(db.read(cursors).unwrap(), command.cursors, "an existing cursor advances too");
    assert!(db.read(|c| lobotomy_core::turn::current_session(c, ROLE, None)).unwrap().is_none());
    send(&db, "m1", None, "arrived after the idle read");
    assert_eq!(rejection(db.execute(&Caller::Runtime, &command)), "manager_state_changed");
    assert_eq!(db.read(|c| queued_messages(c, ROLE)).unwrap().len(), 1);
}
