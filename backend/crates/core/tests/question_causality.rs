mod common;

use common::*;
use lobotomy_core::question::Question;
use lobotomy_core::report::ReportStatus;
use lobotomy_core::task::{SendToRole, load_task};
use lobotomy_core::view::{Attention, overview};
use lobotomy_core::{Caller, Db};

fn message(db: &Db, manager: bool, request: &str, task: &str, body: &str) {
    if manager {
        db.execute(
            &Caller::Manager { turn_id: "angela-turn".into() },
            &SendToRole {
                request_id: request.into(),
                role: ROLE.into(),
                task_id: Some(task.into()),
                quotes: vec![],
                context: vec![],
                ledger: vec![],
                interpretation: body.into(),
            },
        )
        .unwrap();
    } else {
        send(db, request, Some(task), body);
    }
}

#[test]
fn an_older_continue_message_does_not_answer_a_new_question() {
    for manager in [false, true] {
        let db = db();
        let task = create(&db, "task");
        start(&db, &task);
        let first = register(&db);
        message(&db, manager, "old", &task, "继续");
        report(&db, &first.turn_id, ReportStatus::Blocked, Some("修复 MissingSeparator？")).unwrap();
        let waiting = || reply_queued(&db, &task);
        assert!(!waiting(), "an older message is context, not a queued answer (manager={manager})");
        end_turn(&db, &first.turn_id);
        pin_captures(&db);
        let contextual = register(&db);
        let input = db.read(|c| lobotomy_core::turn::load_turn(c, &contextual.turn_id)).unwrap().input;
        assert!(
            input.contains("继续") && input.contains("仅作上下文，未关联当前问题") && input.contains("仍在等待回答"),
            "{input}"
        );
        assert_eq!(
            db.read(|c| load_task(c, &task)).unwrap().blocked_reason.as_deref(),
            Some("修复 MissingSeparator？")
        );
        assert!(attention(&db).iter().any(|a| matches!(a, Attention::TaskBlocked { .. })));
        finish(&db, &contextual.turn_id);
        message(&db, manager, "new", &task, "请修复这个生成器错误");
        // Equal and backwards wall clocks cannot alter causality or input order.
        db.write(|tx| {
            tx.execute("UPDATE question SET created_at = 1000", [])?;
            tx.execute("UPDATE message SET created_at = CASE WHEN seq % 2 = 0 THEN 1000 ELSE 0 END", [])?;
            Ok(())
        })
        .unwrap();
        assert!(waiting());
        assert!(attention(&db).is_empty());
        assert!(db.read(|c| load_task(c, &task)).unwrap().blocked_reason.is_some());
        let answered = register(&db);
        assert!(questions(&db).is_empty());
        assert!(db.read(|c| load_task(c, &task)).unwrap().blocked_reason.is_none());
        let input = db.read(|c| lobotomy_core::turn::load_turn(c, &answered.turn_id)).unwrap().input;
        assert!(input.contains("提问之后的回复，关联 q_") && input.contains("修复 MissingSeparator？"), "{input}");
        let closed_in: String = db
            .read(|c| Ok(c.query_row("SELECT closed_in FROM question WHERE task_id = ?1", [&task], |r| r.get(0))?))
            .unwrap();
        assert_eq!(closed_in, answered.turn_id);
    }
}

fn questions(db: &Db) -> Vec<Question> {
    db.read(|c| lobotomy_core::question::open_questions(c, Some(ROLE))).unwrap()
}

fn reply_queued(db: &Db, task: &str) -> bool {
    db.read(|c| Ok(overview(c, "lobotomy")?.tasks.into_iter().find(|t| t.task.id == task).unwrap().reply_queued))
        .unwrap()
}

fn attention(db: &Db) -> Vec<Attention> {
    db.read(|c| Ok(lobotomy_core::view::attention(&overview(c, "lobotomy")?, &[], &[], |_| false))).unwrap()
}

fn finish(db: &Db, turn: &str) {
    end_turn(db, turn);
    pin_captures(db);
}

#[test]
fn a_question_generation_survives_duplicates_but_not_reasking_or_replacing() {
    use lobotomy_core::report::ReportEffect;
    let db = db();
    let task = create(&db, "task");
    start(&db, &task);
    let first = register(&db);
    report(&db, &first.turn_id, ReportStatus::Blocked, Some("Q1")).unwrap();
    let original = questions(&db)[0].clone();
    send(&db, "reply-q1", Some(&task), "Q1 answer");
    assert_eq!(report(&db, &first.turn_id, ReportStatus::Blocked, Some("Q1")).unwrap(), ReportEffect::Unchanged);
    assert_eq!(questions(&db), [original]);
    assert!(attention(&db).is_empty(), "a duplicate must not invalidate the queued reply");

    report(&db, &first.turn_id, ReportStatus::Blocked, Some("Q2")).unwrap();
    assert!(matches!(attention(&db).as_slice(), [Attention::TaskBlocked { reason, .. }] if reason == "Q2"));
    finish(&db, &first.turn_id);
    let next = register(&db);
    let q2 = questions(&db)[0].clone();
    send(&db, "reply-q2", Some(&task), "Q2 answer");
    assert_eq!(report(&db, &next.turn_id, ReportStatus::Blocked, Some("Q2")).unwrap(), ReportEffect::Recorded);
    let reasked = questions(&db)[0].clone();
    assert_ne!(reasked.id, q2.id, "a new turn explicitly reasks, even with identical words");
    assert!(
        !db.read(|c| Ok(lobotomy_core::task::queued_messages(c, ROLE)?.iter().any(|m| reasked.accepts(m)))).unwrap()
    );
    report(&db, &next.turn_id, ReportStatus::Progress, None).unwrap();
    assert!(questions(&db).is_empty());
    report(&db, &next.turn_id, ReportStatus::Blocked, Some("Q2")).unwrap();
    assert_ne!(questions(&db)[0].id, reasked.id, "withdrawing then asking creates a new generation");
}

#[test]
fn manager_questions_match_individually_by_enqueue_order_and_recipient() {
    use lobotomy_core::manager::AskUser;
    let db = db();
    idle_slot(&db);
    send(&db, "start", None, "看看现状");
    let first = register(&db);
    let caller = Caller::Role { role: ROLE.into(), turn_id: first.turn_id.clone() };
    send(&db, "old", None, "继续");
    let ask = AskUser { question: "Q1".into() };
    let q1 = db.execute(&caller, &ask).unwrap();
    send(&db, "answer", None, "Q1 answer");
    assert_eq!(db.execute(&caller, &ask).unwrap(), q1, "duplicate ask preserves the boundary");
    let q2 = db.execute(&caller, &AskUser { question: "Q2".into() }).unwrap();
    db.execute(
        &Caller::Runtime,
        &lobotomy_core::manager::TellManager {
            role: ROLE.into(),
            body: Some("runtime update".into()),
            cursors: vec![],
        },
    )
    .unwrap();
    // A message to another role, even after Q2, cannot answer it.
    db.write(|tx| {
        tx.execute("INSERT INTO role (name, kind, harness) VALUES ('Other', 'worker', 'codex')", [])?;
        tx.execute("UPDATE question SET created_at = 1000", [])?;
        tx.execute("UPDATE message SET created_at = 1000", [])?;
        Ok(())
    })
    .unwrap();
    db.execute(
        &Caller::User,
        &lobotomy_core::task::SendMessage {
            request_id: "other".into(),
            role: "Other".into(),
            task_id: None,
            body: "answer".into(),
            reply_to: None,
        },
    )
    .unwrap();
    assert!(matches!(attention(&db).as_slice(), [Attention::Question { id, .. }] if id == &q2));
    finish(&db, &first.turn_id);
    let second = register(&db);
    assert_eq!(questions(&db).iter().map(|q| &q.id).collect::<Vec<_>>(), [&q2]);
    let input = db.read(|c| lobotomy_core::turn::load_turn(c, &second.turn_id)).unwrap().input;
    assert!(input.contains(&format!("提问之后的回复，关联 {q1}")) && input.contains("仍在等待回答"), "{input}");
    finish(&db, &second.turn_id);
    send(&db, "answer-q2", None, "Q2 answer");
    assert!(attention(&db).is_empty());
    register(&db);
    assert!(questions(&db).is_empty());
}

#[test]
fn restarting_and_continuing_do_not_turn_context_into_an_answer_or_lift_a_pause() {
    use lobotomy_core::task::SetPaused;
    use lobotomy_core::turn::{Continue, EndTurn, Failure, FailureKind, NewNativeSession, Outcome, RegisterTurn};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("question.db");
    let db = onboard(Db::open(&path).unwrap());
    let task = create(&db, "task");
    start(&db, &task);
    let first = register(&db);
    send(&db, "old", Some(&task), "继续");
    report(&db, &first.turn_id, ReportStatus::Blocked, Some("Q")).unwrap();
    let question = questions(&db)[0].clone();
    finish(&db, &first.turn_id);
    drop(db);
    let db = Db::open(&path).unwrap();
    assert_eq!(questions(&db).as_slice(), std::slice::from_ref(&question));
    db.execute(&Caller::User, &NewNativeSession { request_id: "new-session".into(), role: ROLE.into() }).unwrap();
    let contextual = register(&db);
    assert_eq!(questions(&db).as_slice(), std::slice::from_ref(&question));
    db.execute(
        &Caller::Runtime,
        &EndTurn {
            turn_id: contextual.turn_id.clone(),
            outcome: Outcome::Failed,
            failure: Some(Failure {
                kind: FailureKind::Other,
                message: "did not start".into(),
                resets_at: None,
                unstarted: true,
            }),
        },
    )
    .unwrap();
    pin_captures(&db);
    let resumed = db.execute(&Caller::User, &Continue { request_id: "continue".into(), role: ROLE.into() }).unwrap();
    let input = db.read(|c| lobotomy_core::turn::load_turn(c, &resumed.turn_id)).unwrap().input;
    assert!(input.contains("继续") && input.contains("仍在等待回答"), "{input}");
    assert_eq!(questions(&db), [question]);
    finish(&db, &resumed.turn_id);
    db.execute(&Caller::User, &SetPaused { request_id: "pause".into(), task_id: task.clone(), paused: true }).unwrap();
    send(&db, "answer", Some(&task), "answer Q");
    assert_eq!(rejection(db.execute(&Caller::Runtime, &RegisterTurn { role: ROLE.into() })), "paused");
    assert!(!questions(&db).is_empty());
    assert_eq!(
        rejection(db.execute(
            &Caller::Manager { turn_id: "angela".into() },
            &SetPaused { request_id: "manager-resume".into(), task_id: task.clone(), paused: false }
        )),
        "paused_by_user"
    );
    db.execute(&Caller::User, &SetPaused { request_id: "resume".into(), task_id: task, paused: false }).unwrap();
    register(&db);
    assert!(questions(&db).is_empty());
}

#[test]
fn upgrading_recovers_boundaries_from_events_and_keeps_unknown_history_open() {
    use lobotomy_core::manager::AskUser;
    for (version, known_history) in [(16, false), (16, true), (17, false), (17, true)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("question.db");
        let db = onboard(Db::open(&path).unwrap());
        let task = create(&db, "task");
        start(&db, &task);
        let first = register(&db);
        send(&db, "old", Some(&task), "old");
        report(&db, &first.turn_id, ReportStatus::Blocked, Some("worker Q")).unwrap();
        let asker = Caller::Role { role: ROLE.into(), turn_id: first.turn_id.clone() };
        let manager_q = db.execute(&asker, &AskUser { question: "manager Q".into() }).unwrap();
        send(&db, "answer", None, "answer both");
        finish(&db, &first.turn_id);
        drop(db);
        // Restore the prior schema, including the old task mirror, without losing its data.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "DROP TABLE role_model; ALTER TABLE role ADD COLUMN model TEXT;
            ALTER TABLE task ADD COLUMN blocked_reason TEXT;
            UPDATE task SET blocked_reason = (SELECT body FROM question WHERE task_id = task.id AND closed_at IS NULL);
            PRAGMA user_version = 17;",
        )
        .unwrap();
        if version == 16 {
            conn.execute_batch(
                "DELETE FROM question WHERE task_id IS NOT NULL;
            DROP INDEX question_open_by_role; DROP INDEX question_open_by_task;
            ALTER TABLE question DROP COLUMN role; ALTER TABLE question DROP COLUMN task_id;
            ALTER TABLE question DROP COLUMN after_message_seq; PRAGMA user_version = 16;
            UPDATE message SET created_at = 1; UPDATE event SET created_at = 1; UPDATE question SET created_at = 1;",
            )
            .unwrap();
        }
        if !known_history {
            conn.execute("DELETE FROM event WHERE kind IN ('question.asked', 'task.blocked_changed')", []).unwrap();
        }
        drop(conn);
        let db = Db::open(&path).unwrap();
        assert!(
            db.read(|c| Ok(c.prepare("SELECT blocked_reason FROM task")?.column_count())).is_err(),
            "the mirror column is physically absent"
        );
        assert_eq!(db.read(|c| load_task(c, &task)).unwrap().blocked_reason.as_deref(), Some("worker Q"));
        // The public DTO reads the question itself, not a stale copy in task.
        db.write(|tx| {
            tx.execute("UPDATE question SET body = 'current Q' WHERE task_id = ?1", [&task])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            serde_json::to_value(db.read(|c| load_task(c, &task)).unwrap()).unwrap()["blocked_reason"],
            "current Q"
        );
        let can_answer = version == 17 || known_history;
        assert_eq!(questions(&db).len(), 2);
        let manager = questions(&db).into_iter().find(|q| q.id == manager_q).unwrap();
        assert_eq!(
            db.read(|c| Ok(lobotomy_core::task::queued_messages(c, ROLE)?.iter().any(|m| manager.accepts(m)))).unwrap(),
            can_answer
        );
        assert_eq!(reply_queued(&db, &task), can_answer);
        let delivered = register(&db);
        assert_eq!(questions(&db).is_empty(), can_answer);
        if !can_answer {
            finish(&db, &delivered.turn_id);
            send(&db, "fresh", None, "new answer");
            register(&db);
            assert!(questions(&db).is_empty());
        }
    }
}

#[test]
fn a_message_for_another_task_is_context_but_a_role_wide_reply_can_answer() {
    let db = db();
    let task = create(&db, "task");
    let other = create(&db, "other");
    start(&db, &task);
    let first = register(&db);
    report(&db, &first.turn_id, ReportStatus::Blocked, Some("Q")).unwrap();
    send(&db, "other-reply", Some(&other), "other task answer");
    assert!(!reply_queued(&db, &task));
    finish(&db, &first.turn_id);
    let context = register(&db);
    assert_eq!(questions(&db).len(), 1);
    finish(&db, &context.turn_id);
    send(&db, "role-wide", None, "answer current question");
    assert!(reply_queued(&db, &task));
    register(&db);
    assert!(questions(&db).is_empty());
}
