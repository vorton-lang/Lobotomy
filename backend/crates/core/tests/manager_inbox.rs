//! What wakes the manager and what she is told (manager-actions.md §6 "Manager 什么时候运行";
//! data-model.md §10.6 跨库操作第 4、5 条): a project settles when nothing in it moves on without
//! the user; what changed since her cursor comes in a line each, with the ids she can attach.

mod common;

use common::*;
use lobotomy_core::Caller;
use lobotomy_core::manager::{Cursor, TellManager, cursors};
use lobotomy_core::report::ReportStatus;
use lobotomy_core::task::{SetPaused, queued_messages};
use lobotomy_core::view::last_event;
use lobotomy_core::view::manager::{changes_since, settled};

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
