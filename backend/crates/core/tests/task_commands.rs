use lobotomy_core::task::{
    Abandon, CreateTask, EditCriteria, MoveInQueue, Phase, Reopen, SendMessage, SetPaused, StartAttempt, list_tasks,
    load_task, occupant, queued_messages,
};
use lobotomy_core::{Caller, Db};

fn db() -> Db {
    Db::open_in_memory().unwrap()
}

fn create(db: &Db, request_id: &str, title: &str) -> String {
    db.execute(
        &Caller::User,
        &CreateTask {
            request_id: request_id.into(),
            title: title.into(),
            body: "原话".into(),
            criteria: "测试通过".into(),
            executor: "Malkuth".into(),
        },
    )
    .unwrap()
    .id
}

fn task(db: &Db, id: &str) -> lobotomy_core::task::Task {
    db.read(|c| load_task(c, id)).unwrap()
}

fn rejection<T: std::fmt::Debug>(r: lobotomy_core::Result<T>) -> &'static str {
    r.unwrap_err().code().expect("expected a rejection")
}

#[test]
fn create_task_is_idempotent_per_request_id() {
    let db = db();
    let a = create(&db, "r1", "实现 adapter");
    let b = create(&db, "r1", "实现 adapter");
    assert_eq!(a, b);
    assert_eq!(db.read(list_tasks).unwrap().len(), 1);
    let t = task(&db, &a);
    assert_eq!((t.phase, t.criteria_version, t.queue_pos), (Phase::Queued, 1, Some(1)));
}

#[test]
fn reusing_a_key_for_another_command_is_rejected() {
    let db = db();
    let id = create(&db, "r1", "t");
    let r = db.execute(&Caller::User, &SetPaused { request_id: "r1".into(), task_id: id, paused: true });
    assert_eq!(rejection(r), "idempotency_key_reused");
}

#[test]
fn only_the_user_can_create_tasks_and_only_the_runtime_starts_attempts() {
    let db = db();
    let role = Caller::Role { role: "Malkuth".into(), turn_id: "turn_x".into() };
    let r = db.execute(
        &role,
        &CreateTask {
            request_id: "r1".into(),
            title: "t".into(),
            body: String::new(),
            criteria: String::new(),
            executor: "Malkuth".into(),
        },
    );
    assert_eq!(rejection(r), "forbidden");
    let id = create(&db, "r2", "t");
    assert_eq!(rejection(db.execute(&Caller::User, &StartAttempt { task_id: id })), "forbidden");
}

#[test]
fn unknown_executor_and_empty_title_are_rejected() {
    let db = db();
    let mut cmd = CreateTask {
        request_id: "r1".into(),
        title: "t".into(),
        body: String::new(),
        criteria: String::new(),
        executor: "Nobody".into(),
    };
    assert_eq!(rejection(db.execute(&Caller::User, &cmd)), "unknown_role");
    cmd.executor = "Malkuth".into();
    cmd.title = "  ".into();
    assert_eq!(rejection(db.execute(&Caller::User, &cmd)), "empty_title");
}

#[test]
fn starting_an_attempt_occupies_the_executor() {
    let db = db();
    let a = create(&db, "r1", "a");
    let b = create(&db, "r2", "b");
    let started = db.execute(&Caller::Runtime, &StartAttempt { task_id: a.clone() }).unwrap();
    assert_eq!(started.seq, 1);
    assert_eq!(task(&db, &a).phase, Phase::Executing);
    assert_eq!(db.read(|c| occupant(c, "Malkuth")).unwrap(), Some(a.clone()));
    // The executor is busy, so the second task cannot start.
    assert_eq!(rejection(db.execute(&Caller::Runtime, &StartAttempt { task_id: b })), "role_busy");
    // A task in execution cannot start another attempt.
    assert_eq!(rejection(db.execute(&Caller::Runtime, &StartAttempt { task_id: a })), "not_queued");
}

#[test]
fn the_database_allows_one_open_attempt_per_task_and_one_occupant_per_role() {
    let db = db();
    let a = create(&db, "r1", "a");
    let b = create(&db, "r2", "b");
    db.execute(&Caller::Runtime, &StartAttempt { task_id: a.clone() }).unwrap();
    db.read(|c| {
        let second_open = c.execute(
            "INSERT INTO attempt (id, task_id, seq, started_at) VALUES ('att_dup', ?1, 2, 0)",
            [&a],
        );
        assert!(second_open.is_err(), "a second open attempt must violate the unique index");
        let second_occupant =
            c.execute("INSERT INTO occupancy (role, task_id, since) VALUES ('Malkuth', ?1, 0)", [&b]);
        assert!(second_occupant.is_err(), "a second occupant must violate the primary key");
        Ok(())
    })
    .unwrap();
}

#[test]
fn paused_tasks_do_not_start() {
    let db = db();
    let a = create(&db, "r1", "a");
    db.execute(&Caller::User, &SetPaused { request_id: "p1".into(), task_id: a.clone(), paused: true }).unwrap();
    assert_eq!(rejection(db.execute(&Caller::Runtime, &StartAttempt { task_id: a.clone() })), "paused");
    db.execute(&Caller::User, &SetPaused { request_id: "p2".into(), task_id: a.clone(), paused: false }).unwrap();
    db.execute(&Caller::Runtime, &StartAttempt { task_id: a }).unwrap();
}

#[test]
fn editing_criteria_checks_the_expected_version() {
    let db = db();
    let a = create(&db, "r1", "a");
    let edit = |request_id: &str, expected_version| {
        db.execute(
            &Caller::User,
            &EditCriteria {
                request_id: request_id.into(),
                task_id: a.clone(),
                expected_version,
                text: "新的条件".into(),
            },
        )
    };
    assert_eq!(edit("e1", 1).unwrap().version, 2);
    assert_eq!(rejection(edit("e2", 1)), "stale_version");
    // Before work starts the executor is not told; the new version is part of the task.
    assert!(db.read(|c| queued_messages(c, "Malkuth")).unwrap().is_empty());
}

#[test]
fn editing_criteria_during_execution_messages_the_executor() {
    let db = db();
    let a = create(&db, "r1", "a");
    db.execute(&Caller::Runtime, &StartAttempt { task_id: a.clone() }).unwrap();
    db.execute(
        &Caller::User,
        &EditCriteria { request_id: "e1".into(), task_id: a.clone(), expected_version: 1, text: "新的条件".into() },
    )
    .unwrap();
    let inbox = db.read(|c| queued_messages(c, "Malkuth")).unwrap();
    assert_eq!(inbox.len(), 1);
    assert!(inbox[0].body.contains("第 2 版"));
    assert_eq!(inbox[0].task_id.as_deref(), Some(a.as_str()));
}

#[test]
fn messages_keep_arrival_order() {
    let db = db();
    for (i, body) in ["一", "二", "三"].iter().enumerate() {
        db.execute(
            &Caller::User,
            &SendMessage { request_id: format!("m{i}"), role: "Malkuth".into(), task_id: None, body: (*body).into() },
        )
        .unwrap();
    }
    let bodies: Vec<_> = db.read(|c| queued_messages(c, "Malkuth")).unwrap().into_iter().map(|m| m.body).collect();
    assert_eq!(bodies, ["一", "二", "三"]);
}

#[test]
fn moving_in_the_queue_renumbers_positions() {
    let db = db();
    let a = create(&db, "r1", "a");
    let b = create(&db, "r2", "b");
    let c = create(&db, "r3", "c");
    db.execute(&Caller::User, &MoveInQueue { request_id: "q1".into(), task_id: c.clone(), to_index: 0 }).unwrap();
    let pos = |id: &str| task(&db, id).queue_pos.unwrap();
    assert_eq!((pos(&c), pos(&a), pos(&b)), (1, 2, 3));
}

#[test]
fn abandoning_closes_the_attempt_and_releases_the_executor() {
    let db = db();
    let a = create(&db, "r1", "a");
    db.execute(&Caller::Runtime, &StartAttempt { task_id: a.clone() }).unwrap();
    db.execute(&Caller::User, &Abandon { request_id: "x1".into(), task_id: a.clone(), reason: "方向错了".into() })
        .unwrap();
    let t = task(&db, &a);
    assert_eq!(t.phase, Phase::Abandoned);
    assert!(t.closed_at.is_some());
    assert_eq!(db.read(|c| occupant(c, "Malkuth")).unwrap(), None);
    let open: i64 = db
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM attempt WHERE ended_at IS NULL", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(open, 0);
    // A closed task cannot be abandoned again.
    let again = db.execute(&Caller::User, &Abandon { request_id: "x2".into(), task_id: a, reason: String::new() });
    assert_eq!(rejection(again), "task_closed");
}

#[test]
fn reopening_queues_the_task_and_the_next_attempt_gets_a_new_number() {
    let db = db();
    let a = create(&db, "r1", "a");
    assert_eq!(rejection(db.execute(&Caller::User, &Reopen { request_id: "o0".into(), task_id: a.clone() })), "not_closed");
    db.execute(&Caller::Runtime, &StartAttempt { task_id: a.clone() }).unwrap();
    db.execute(&Caller::User, &Abandon { request_id: "x1".into(), task_id: a.clone(), reason: String::new() }).unwrap();
    db.execute(&Caller::User, &Reopen { request_id: "o1".into(), task_id: a.clone() }).unwrap();
    assert_eq!(task(&db, &a).phase, Phase::Queued);
    let second = db.execute(&Caller::Runtime, &StartAttempt { task_id: a }).unwrap();
    assert_eq!(second.seq, 2);
}

#[test]
fn every_change_lands_in_the_event_log_in_order() {
    let db = db();
    let a = create(&db, "r1", "a");
    db.execute(&Caller::Runtime, &StartAttempt { task_id: a }).unwrap();
    let kinds: Vec<String> = db
        .read(|c| {
            let mut stmt = c.prepare("SELECT kind FROM event ORDER BY seq")?;
            Ok(stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    assert_eq!(kinds, ["task.created", "attempt.started"]);
}
