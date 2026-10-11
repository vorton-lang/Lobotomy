//! The manager's commands in a project (data-model.md §10.6 跨库操作第 2 条; roles-and-tasks.md §4;
//! M4 step 2): what she may run, what an assignment must say, and the user's pause that only the
//! user lifts.

mod common;

use common::*;
use lobotomy_core::report::ReportStatus;
use lobotomy_core::task::{
    Action, Assign, CreateTask, EditCriteria, Quote, Reopen, SendToRole, SetPaused, load_task, queued_messages,
};
use lobotomy_core::verify::{Accept, SendBack};
use lobotomy_core::{Caller, Db};

fn angela() -> Caller {
    Caller::Manager { turn_id: "turn_of_angela".into() }
}

fn quote(id: &str, body: &str) -> Quote {
    Quote { source_id: id.into(), said_at: 1_760_000_000_000, body: body.into() }
}

fn assign(request_id: &str) -> Assign {
    Assign {
        request_id: request_id.into(),
        executor: ROLE.into(),
        title: "查清启动慢的原因".into(),
        quotes: vec![quote("msg_user1", "启动太慢了，先看看为什么")],
        interpretation: "先定位，不改代码".into(),
        actions: vec![Action::ReadOnly],
        stop_when: "找到第一个确定的原因就汇报".into(),
        done_when: "说明原因和证据".into(),
    }
}

fn pause(db: &Db, caller: &Caller, key: &str, task: &str, paused: bool) -> lobotomy_core::Result<()> {
    db.execute(caller, &SetPaused { request_id: key.into(), task_id: task.into(), paused })
}

fn paused_by(db: &Db, task: &str) -> (bool, Option<String>) {
    let t = db.read(|c| load_task(c, task)).unwrap();
    (t.paused, t.paused_by)
}

#[test]
fn the_manager_runs_the_control_commands_and_the_user_keeps_the_rest() {
    let db = db();
    let create = CreateTask {
        request_id: "c1".into(),
        title: "t".into(),
        body: String::new(),
        criteria: "x".into(),
        executor: ROLE.into(),
    };
    assert_eq!(rejection(db.execute(&angela(), &create)), "forbidden", "she assigns instead");
    let task = create_titled(&db, "c2", "t");
    let edit = EditCriteria { request_id: "e1".into(), task_id: task.clone(), expected_version: 1, text: "y".into() };
    assert_eq!(rejection(db.execute(&angela(), &edit)), "forbidden");
    let reopen = Reopen { request_id: "r1".into(), task_id: task.clone() };
    assert_eq!(rejection(db.execute(&angela(), &reopen)), "forbidden");
    // Assigning and forwarding the user's words are hers: the user writes a task or a message.
    assert_eq!(rejection(db.execute(&Caller::User, &assign("a1"))), "forbidden");
    let role = Caller::Role { role: ROLE.into(), turn_id: "turn_x".into() };
    assert_eq!(rejection(pause(&db, &role, "p1", &task, true)), "forbidden");
}

/// #50: the user paused the task after agreeing to go on; the manager, acting on what she read
/// before, cannot start it again (roles-and-tasks.md §4).
#[test]
fn only_the_user_lifts_the_users_pause() {
    let db = db();
    let task = create(&db, "c1");
    pause(&db, &Caller::User, "p1", &task, true).unwrap();
    assert_eq!(rejection(pause(&db, &angela(), "p2", &task, false)), "paused_by_user");
    // Pausing it again does not make the pause hers.
    pause(&db, &angela(), "p3", &task, true).unwrap();
    assert_eq!(paused_by(&db, &task), (true, Some("user".into())));
    assert_eq!(rejection(pause(&db, &angela(), "p4", &task, false)), "paused_by_user");
    pause(&db, &Caller::User, "p5", &task, false).unwrap();
    assert_eq!(paused_by(&db, &task), (false, None));

    // Her own pause she lifts; once the user pauses it too, it is the user's.
    pause(&db, &angela(), "p6", &task, true).unwrap();
    assert_eq!(paused_by(&db, &task), (true, Some("manager".into())));
    pause(&db, &angela(), "p7", &task, false).unwrap();
    pause(&db, &angela(), "p8", &task, true).unwrap();
    pause(&db, &Caller::User, "p9", &task, true).unwrap();
    assert_eq!(rejection(pause(&db, &angela(), "p10", &task, false)), "paused_by_user");
}

#[test]
fn an_assignment_says_what_it_allows_and_when_to_stop() {
    let db = db();
    let mut cmd = assign("a1");
    cmd.actions.clear();
    assert_eq!(rejection(db.execute(&angela(), &cmd)), "no_actions");
    let mut cmd = assign("a2");
    cmd.stop_when = " ".into();
    assert_eq!(rejection(db.execute(&angela(), &cmd)), "missing_field");

    let task = db.execute(&angela(), &assign("a3")).unwrap().id;
    let t = db.read(|c| load_task(c, &task)).unwrap();
    assert!(t.body.starts_with("【用户原话】") && t.body.ends_with("启动太慢了，先看看为什么"), "{}", t.body);
    let a = t.assignment.expect("the assignment is kept");
    assert_eq!((a.actions, a.stop_when.as_str()), (vec![Action::ReadOnly], "找到第一个确定的原因就汇报"));
    // The quote keeps where it came from (data-model.md §10.6 跨库操作第 1 条).
    let source: String =
        db.read(|c| Ok(c.query_row("SELECT source_id FROM quote WHERE task_id = ?1", [&task], |r| r.get(0))?)).unwrap();
    assert_eq!(source, "msg_user1");

    // The executor's brief carries all of it.
    start(&db, &task);
    let brief = db.read(|c| queued_messages(c, ROLE)).unwrap().pop().unwrap().body;
    for part in [
        "由 Angela 派出",
        "启动太慢了",
        "【Angela 解读】\n先定位，不改代码",
        "允许的动作：只读调查",
        "停止条件：找到第一个",
    ] {
        assert!(brief.contains(part), "{part} is missing from {brief}");
    }
}

/// The manager is the executor's user (manager-actions.md §0): her message answers its question as
/// the user's does (#17), and says which words are the user's.
#[test]
fn the_managers_message_quotes_the_user_and_answers_a_question() {
    let db = db();
    let task = create(&db, "c1");
    start(&db, &task);
    let t = register(&db);
    report(&db, &t.turn_id, ReportStatus::Blocked, Some("用哪个端口？")).unwrap();
    end_turn(&db, &t.turn_id);
    pin_captures(&db);
    let message = SendToRole {
        request_id: "s1".into(),
        role: ROLE.into(),
        task_id: Some(task.clone()),
        quotes: vec![quote("msg_user2", "用 8080")],
        interpretation: "端口定为 8080，其余照旧".into(),
    };
    db.execute(&angela(), &message).unwrap();
    let t = register(&db);
    let input: String =
        db.read(|c| Ok(c.query_row("SELECT input FROM turn WHERE id = ?1", [&t.turn_id], |r| r.get(0))?)).unwrap();
    assert!(input.contains("【来自 Angela · 任务") && input.contains("【用户原话】"), "{input}");
    assert!(input.contains("用 8080\n\n【Angela 解读】\n端口定为 8080"), "{input}");
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().blocked_reason, None);
}

/// She accepts and sends back as the user does; the user's words the executor never got are the
/// user's to let go (data-model.md §4.2, #14).
#[test]
fn the_manager_accepts_but_never_lets_the_users_words_go() {
    let db = db();
    let (task, commit) = candidate(&db);
    let v = verify(&db, &task, &commit, vec![], vec![]);
    let added = send(&db, "m1", Some(&task), "异常行要能看到原始行号");
    let accept = |dropping: Vec<String>| Accept {
        request_id: format!("a{}", dropping.len()),
        task_id: task.clone(),
        verification_id: v.clone(),
        criteria_version: 1,
        expected_integration: BASE.into(),
        dropping,
    };
    assert_eq!(rejection(db.execute(&angela(), &accept(vec![]))), "undelivered_messages");
    assert_eq!(rejection(db.execute(&angela(), &accept(vec![added]))), "undelivered_messages");

    db.execute(&angela(), &SendBack { request_id: "b1".into(), task_id: task.clone(), reason: "补上行号".into() })
        .unwrap();
    let sent: Vec<String> = db.read(|c| queued_messages(c, ROLE)).unwrap().into_iter().map(|m| m.body).collect();
    assert!(sent.iter().any(|b| b.starts_with("Angela退回了候选成果：补上行号")), "{sent:?}");
}
