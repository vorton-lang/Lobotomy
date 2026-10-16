use lobotomy_core::task::{MoveInQueue, SetPaused, load_task};
use lobotomy_core::{Caller, Db};

mod common;
use common::{create_titled as create, db, rejection, start};

fn task(db: &Db, id: &str) -> lobotomy_core::task::Task {
    db.read(|c| load_task(c, id)).unwrap()
}

fn event_count(db: &Db) -> i64 {
    db.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0))?)).unwrap()
}

/// (queue position, revision) of each task.
fn queue_state(db: &Db, ids: &[&String]) -> Vec<(i64, i64)> {
    ids.iter().map(|id| task(db, id)).map(|t| (t.queue_pos.unwrap(), t.revision)).collect()
}

#[test]
fn a_retry_with_the_same_arguments_replays_the_first_result() {
    let db = db();
    let a = create(&db, "r1", "a");
    let pause = SetPaused { request_id: "k".into(), task_id: a.clone(), paused: true };
    db.execute(&Caller::User, &pause).unwrap();
    let (revision, events) = (task(&db, &a).revision, event_count(&db));
    db.execute(&Caller::User, &pause).unwrap();
    assert_eq!((task(&db, &a).revision, event_count(&db)), (revision, events));
}

#[test]
fn reusing_a_key_with_other_arguments_is_rejected_without_side_effects() {
    // Issue #9, part 1.
    let db = db();
    let a = create(&db, "r1", "a");
    let b = create(&db, "r2", "b");
    db.execute(&Caller::User, &SetPaused { request_id: "k".into(), task_id: a, paused: true }).unwrap();
    let (before, events) = (task(&db, &b), event_count(&db));
    let r = db.execute(&Caller::User, &SetPaused { request_id: "k".into(), task_id: b.clone(), paused: true });
    assert_eq!(rejection(r), "idempotency_key_reused");
    assert_eq!(task(&db, &b), before);
    assert_eq!(event_count(&db), events);
}

#[test]
fn moving_in_the_queue_updates_every_task_whose_position_changes() {
    // Issue #9, part 2.
    let db = db();
    let a = create(&db, "r1", "a");
    let b = create(&db, "r2", "b");
    let c = create(&db, "r3", "c");
    let d = create(&db, "r4", "d");
    let events = event_count(&db);
    let move_c = MoveInQueue { request_id: "q1".into(), task_id: c.clone(), to_index: 0 };
    db.execute(&Caller::User, &move_c).unwrap();
    // C moves; A and B are pushed back; D keeps its place and its revision.
    assert_eq!(queue_state(&db, &[&c, &a, &b, &d]), [(1, 2), (2, 2), (3, 2), (4, 1)]);
    assert_eq!(event_count(&db), events + 1);

    // Replaying the same request changes nothing.
    db.execute(&Caller::User, &move_c).unwrap();
    assert_eq!(queue_state(&db, &[&c, &a, &b, &d]), [(1, 2), (2, 2), (3, 2), (4, 1)]);
    assert_eq!(event_count(&db), events + 1);
}

#[test]
fn moving_a_task_to_its_own_place_changes_nothing() {
    let db = db();
    let a = create(&db, "r1", "a");
    let b = create(&db, "r2", "b");
    let events = event_count(&db);
    db.execute(&Caller::User, &MoveInQueue { request_id: "q1".into(), task_id: b.clone(), to_index: 1 }).unwrap();
    assert_eq!(queue_state(&db, &[&a, &b]), [(1, 1), (2, 1)]);
    assert_eq!(event_count(&db), events);
}

#[test]
fn moving_in_the_queue_reuses_existing_positions() {
    let db = db();
    let a = create(&db, "r1", "a");
    let b = create(&db, "r2", "b");
    let c = create(&db, "r3", "c");
    let d = create(&db, "r4", "d");
    // A leaves the queue, so positions start at 2.
    start(&db, &a);
    db.execute(&Caller::User, &MoveInQueue { request_id: "q1".into(), task_id: d.clone(), to_index: 0 }).unwrap();
    assert_eq!(queue_state(&db, &[&d, &b, &c]), [(2, 2), (3, 2), (4, 2)]);
}
