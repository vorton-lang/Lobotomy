//! What wakes the manager and what she is told (manager-actions.md §6 "Manager 什么时候运行";
//! data-model.md §10.6 跨库操作第 4、5 条): a project settles when nothing in it moves on without
//! the user; what changed since her cursor comes in a line each, with the ids she can attach.

mod common;

use common::*;
use lobotomy_core::Caller;
use lobotomy_core::manager::{AskUser, Cursor, ScheduleManagerTurn, TellManager, cursors, open_questions};
use lobotomy_core::report::{FlagMismatch, ReportStatus};
use lobotomy_core::task::{SendMessage, SetPaused, queued_messages};
use lobotomy_core::verify::{FinishVerification, StartVerification};
use lobotomy_core::view::manager::{changes_since, settled};
use lobotomy_core::view::{Attention, attention, last_event, overview};

fn is_settled(db: &lobotomy_core::Db, blocked: &[&str]) -> bool {
    let blocked: Vec<String> = blocked.iter().map(|h| h.to_string()).collect();
    db.read(|c| settled(c, &blocked)).unwrap()
}

#[test]
fn a_project_settles_when_nothing_moves_on_without_the_user() {
    let db = db();
    assert!(is_settled(&db, &[]), "nothing to do");
    let task = create(&db, "c1");
    assert!(!is_settled(&db, &[]), "the free executor starts the task");
    start(&db, &task);
    assert!(!is_settled(&db, &[]), "the brief goes in the next turn");
    assert!(is_settled(&db, &["codex"]), "unless the executor's quota is blocked");
    let t = register(&db);
    assert!(!is_settled(&db, &[]), "a turn runs");
    report(&db, &t.turn_id, ReportStatus::Blocked, Some("用哪个端口？")).unwrap();
    end_turn(&db, &t.turn_id);
    assert!(!is_settled(&db, &[]), "the turn's capture is pending");
    pin_captures(&db);
    assert!(is_settled(&db, &[]), "the executor waits for the user's answer");
    send(&db, "m1", Some(&task), "用 8080");
    assert!(!is_settled(&db, &[]), "the answer goes in the next turn");
    db.execute(&Caller::User, &SetPaused { request_id: "p1".into(), task_id: task, paused: true }).unwrap();
    assert!(is_settled(&db, &[]), "a paused task's messages wait");
}

/// #62: a paused task waits in verification for the user; only checks already running move on.
#[test]
fn a_paused_task_waiting_in_verification_settles_once_its_checks_end() {
    let db = db();
    let (task, commit) = candidate(&db);
    assert!(!is_settled(&db, &[]), "the candidate is verified");
    let v = db.execute(&Caller::Runtime, &StartVerification { task_id: task.clone() }).unwrap();
    let pause = |key: &str, paused| SetPaused { request_id: key.into(), task_id: task.clone(), paused };
    db.execute(&Caller::User, &pause("p1", true)).unwrap();
    assert!(!is_settled(&db, &[]), "the checks still run");
    let finish = FinishVerification { verification_id: v, commit, conflicts: vec![], checks: vec![], blobs: vec![] };
    db.execute(&Caller::Runtime, &finish).unwrap();
    assert!(is_settled(&db, &[]), "the paused task waits for the user");
    db.execute(&Caller::User, &pause("p2", false)).unwrap();
    assert!(!is_settled(&db, &[]), "the resumed task is verified again");
}

#[test]
fn what_changed_comes_in_a_line_each_with_the_ids_to_attach() {
    let db = db();
    let (task, commit) = candidate(&db);
    let v = verify(&db, &task, &commit, vec![], vec![]);
    send(&db, "m1", Some(&task), "导出要带表头\n第二行不进摘要");
    let read = db.read(|c| changes_since(c, 0)).unwrap();
    assert_eq!((read.after, read.as_of, read.not_listed), (0, db.read(last_event).unwrap(), 0));
    assert_eq!(read.by_others, read.lines.len(), "nothing here is hers");
    let all = read.lines.join("\n");
    for part in [
        format!("用户建了任务 「加一个 new.txt」（{task}），执行者 Malkuth"),
        "Malkuth 汇报（完成）：完成了，任务".to_owned(),
        "；汇报 cmd_".to_owned(),
        format!("的验证通过（{v}）"),
        "等待验收".to_owned(),
        "用户直接给 Malkuth 发了消息：导出要带表头".to_owned(),
    ] {
        assert!(all.contains(&part), "{part} is missing from:\n{all}");
    }
    assert!(!all.contains("第二行"), "{all}");
    // From her cursor on, only what came after.
    let later = db.read(|c| changes_since(c, read.as_of)).unwrap();
    assert_eq!((later.lines.len(), later.as_of), (0, read.as_of));
    // What she did herself is listed, and is nothing new to her.
    let angela = Caller::Manager { turn_id: "turn_of_angela".into() };
    db.execute(&angela, &SetPaused { request_id: "p1".into(), task_id: task.clone(), paused: true }).unwrap();
    let hers = db.read(|c| changes_since(c, read.as_of)).unwrap();
    assert_eq!((hers.lines.len(), hers.by_others), (1, 0));
    assert!(hers.lines[0].starts_with("你暂停了任务"), "{:?}", hers.lines);
}

/// Her question waits in "等你决定" until a message the user sent after it is delivered to her;
/// one sent before it, still queued, answers nothing (manager-actions.md §6 `ask_user`). The role
/// stands in for her: her database has the projects' schema.
#[test]
fn her_question_waits_for_a_message_sent_after_it() {
    let db = db();
    idle_slot(&db);
    send(&db, "m1", None, "看看现状");
    let t1 = register(&db);
    send(&db, "m2", None, "顺便问一下进度");
    let asker = Caller::Role { role: ROLE.into(), turn_id: t1.turn_id.clone() };
    let ask = AskUser { question: "导出先做 CSV 还是 Excel？".into() };
    let id = db.execute(&asker, &ask).unwrap();
    let open = || db.read(open_questions).unwrap();
    assert_eq!(open()[0].id, id);
    let overview = db.read(|c| overview(c, "lobotomy")).unwrap();
    let asked = attention(&overview, &[], &[], |_| false);
    assert!(asked.iter().any(|a| matches!(a, Attention::Question { question, .. } if question.starts_with("导出"))));
    end_turn(&db, &t1.turn_id);
    pin_captures(&db);
    assert_eq!(common::rejection(db.execute(&asker, &ask)), "turn_ended");

    let t2 = register(&db);
    assert_eq!(open().len(), 1, "m2 was sent before the question");
    end_turn(&db, &t2.turn_id);
    pin_captures(&db);
    send(&db, "m3", None, "先做 CSV");
    register(&db);
    assert!(open().is_empty(), "the answer closes it");
}

/// A role's word that her interpretation differs from the user's comes apart from the other
/// changes, whole (manager-actions.md §2.3), and only from a turn still running.
#[test]
fn a_mismatch_is_told_apart_from_the_changes() {
    let db = db();
    let task = create(&db, "c1");
    start(&db, &task);
    let t = register(&db);
    let role = Caller::Role { role: ROLE.into(), turn_id: t.turn_id.clone() };
    let flag = FlagMismatch { body: "原话说先别大改，解读要求重写内存模型".into() };
    db.execute(&role, &flag).unwrap();
    let read = db.read(|c| changes_since(c, 0)).unwrap();
    assert_eq!(read.mismatches, ["Malkuth（任务「加一个 new.txt」）：原话说先别大改，解读要求重写内存模型"]);
    assert!(read.lines.iter().all(|l| !l.contains("原话说先别大改")), "{:?}", read.lines);
    end_turn(&db, &t.turn_id);
    assert_eq!(common::rejection(db.execute(&role, &flag)), "turn_ended");
}

/// The user chose what a message replies to: it must exist, and the reader's input says what it is
/// (manager-actions.md §1.1).
#[test]
fn a_reply_says_what_it_replies_to() {
    let db = db();
    idle_slot(&db);
    let earlier = send(&db, "m1", None, "导出要不要带表头？\n第二行");
    let reply = |request_id: &str, reply_to: &str| SendMessage {
        request_id: request_id.into(),
        role: ROLE.into(),
        task_id: None,
        body: "要".into(),
        reply_to: Some(reply_to.into()),
    };
    assert_eq!(common::rejection(db.execute(&Caller::User, &reply("r0", "item_none"))), "not_found");
    db.execute(&Caller::User, &reply("r1", &earlier)).unwrap();
    let t = register(&db);
    let input: String =
        db.read(|c| Ok(c.query_row("SELECT input FROM turn WHERE id = ?1", [&t.turn_id], |r| r.get(0))?)).unwrap();
    assert!(
        input.contains(&format!("【来自 用户】\n（回复 用户 的 {earlier}：「导出要不要带表头？」）\n要")),
        "{input}"
    );
}

/// Her inbox and her cursors are written together, in her database only (data-model.md §10.6
/// 跨库操作第 5 条).
#[test]
fn telling_the_manager_moves_her_cursors_with_the_message() {
    let db = db();
    let cursor = |seq| Cursor { project_id: "prj_a".into(), seq };
    let first = TellManager { role: ROLE.into(), body: None, cursors: vec![cursor(4)] };
    assert_eq!(db.execute(&Caller::Runtime, &first).unwrap(), None);
    let told = TellManager { role: ROLE.into(), body: Some("alpha 全员在等你".into()), cursors: vec![cursor(9)] };
    let message = db.execute(&Caller::Runtime, &told).unwrap().expect("a message");
    assert_eq!(db.read(cursors).unwrap(), [cursor(9)]);
    let inbox = db.read(|c| queued_messages(c, ROLE)).unwrap();
    assert_eq!((inbox[0].id.as_str(), inbox[0].source.as_str()), (message.as_str(), "runtime"));
    assert_eq!(common::rejection(db.execute(&Caller::User, &told)), "forbidden");
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
    idle_slot(&db);
    let said = send(&db, "m1", None, "user message");
    let command = prepared(&db);
    let turn = db.execute(&Caller::Runtime, &command).unwrap().unwrap();
    let input = db.read(|c| lobotomy_core::turn::load_turn(c, &turn.turn_id)).unwrap().input;
    for text in ["user message", "ledger for this session", "project notice"] {
        assert!(input.contains(text), "{input}");
    }
    assert_eq!(db.read(cursors).unwrap(), command.cursors);
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
        idle_slot(&db);
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
    idle_slot(&db);
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
    let command = ScheduleManagerTurn { messages: vec![], register: false, ..prepared(&db) };
    assert!(db.execute(&Caller::Runtime, &command).unwrap().is_none());
    assert!(db.execute(&Caller::Runtime, &command).unwrap().is_none());
    assert_eq!(db.read(cursors).unwrap(), command.cursors);
    assert!(db.read(|c| lobotomy_core::turn::current_session(c, ROLE, None)).unwrap().is_none());
    send(&db, "m1", None, "arrived after the idle read");
    assert_eq!(rejection(db.execute(&Caller::Runtime, &command)), "manager_state_changed");
    assert_eq!(db.read(|c| queued_messages(c, ROLE)).unwrap().len(), 1);
}
