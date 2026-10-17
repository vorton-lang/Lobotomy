//! A task a worker runs, as the orchestrator keeps it from the worker's events (tech-plan.md §7
//! step 1; worker-interface.md §9).

mod common;

use common::*;
use lobotomy_core::role::SyncWorkerRoles;
use lobotomy_core::task::{CreateTask, Phase, SendMessage, SendToRole, load_task};
use lobotomy_core::view::manager::settled;
use lobotomy_core::worker::{ApplyWorkerEvent, ApplyWorkerSnapshot, CloseWorkerTask, WorkerTaskCreated, worker_task};
use lobotomy_core::{Caller, Db, Error};
use lobotomy_worker_protocol::event::{
    Artifact, ArtifactKind, AttemptState, Blocked, Event, EventBody, Need, TaskSnapshot,
};
use lobotomy_worker_protocol::ops::CloseOutcome;

const WORKER: &str = "lobotomy-codex";

fn event(seq: u64, attempt: &str, body: EventBody) -> Event {
    Event { task_id: "worker's".into(), attempt_id: attempt.into(), seq, body }
}

fn state(state: AttemptState) -> EventBody {
    EventBody::StateChanged { state, blocked: None, reason: None }
}

/// A task of the worker's, before the worker has it.
fn queued_task(db: &Db) -> String {
    let workers = SyncWorkerRoles { workers: vec![(WORKER.into(), "codex".into())] };
    db.execute(&Caller::Runtime, &workers).unwrap();
    let create = CreateTask {
        request_id: "t".into(),
        title: "加一个 new.txt".into(),
        body: "原话".into(),
        criteria: "CI 通过".into(),
        executor: WORKER.into(),
    };
    db.execute(&Caller::User, &create).unwrap().id
}

fn hand_over(db: &Db, task: &str) {
    let created =
        WorkerTaskCreated { task_id: task.into(), worker: WORKER.into(), spec_version: 1, attempt_id: "a1".into() };
    db.execute(&Caller::Runtime, &created).unwrap();
}

#[test]
fn the_worker_events_move_the_task_once_each_and_the_snapshot_wins() {
    let db = db();
    let task = queued_task(&db);
    assert!(!db.read(|c| settled(c, &[])).unwrap(), "a queued task waits to go to its worker");
    hand_over(&db, &task);
    let apply = |e: Event| db.execute(&Caller::Runtime, &ApplyWorkerEvent { task_id: task.clone(), event: e }).unwrap();

    assert!(apply(event(1, "a1", state(AttemptState::Working))));
    assert!(!db.read(|c| settled(c, &[])).unwrap(), "a working attempt is on its way");
    let change = Artifact {
        artifact_id: "a1:change".into(),
        version: "abc".into(),
        kind: ArtifactKind::Change,
        supersedes: None,
        title: "加一个 new.txt".into(),
        url: Some("https://example.test/pull/1".into()),
        size: None,
        text: None,
        files: vec!["new.txt".into()],
    };
    assert!(apply(event(2, "a1", EventBody::ArtifactPublished { artifact: change })));
    assert!(apply(event(3, "a1", state(AttemptState::Delivered))));
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, Phase::Accepting);
    assert!(db.read(|c| settled(c, &[])).unwrap());
    // Applied once: a resent event changes nothing.
    assert!(!apply(event(3, "a1", state(AttemptState::Working))));
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, Phase::Accepting);

    // Review rework executes again; an earlier attempt's event moves nothing.
    assert!(apply(event(4, "a1", state(AttemptState::Working))));
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, Phase::Executing);
    assert!(apply(event(5, "a0", state(AttemptState::Delivered))));
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, Phase::Executing);

    // After a restart, the snapshot is what holds.
    let snapshot = TaskSnapshot {
        task_id: task.clone(),
        spec_version: 1,
        attempt_id: "a1".into(),
        state: AttemptState::Delivered,
        blocked: None,
        reason: None,
        closed: None,
        question: None,
        messages: Vec::new(),
        artifacts: Vec::new(),
        evidence: Vec::new(),
        seq: 9,
    };
    db.execute(&Caller::Runtime, &ApplyWorkerSnapshot { snapshot: snapshot.clone() }).unwrap();
    let worker = db.read(|c| worker_task(c, &task)).unwrap().unwrap();
    assert_eq!(
        (worker.status.state, worker.seq),
        (AttemptState::Delivered, 5),
        "the snapshot holds; the seq waits for the resent events"
    );
    assert_eq!(worker.change().unwrap().version, "abc");
    assert!(apply(event(9, "a1", state(AttemptState::Working))));
    let stale = TaskSnapshot { seq: 8, ..snapshot.clone() };
    db.execute(&Caller::Runtime, &ApplyWorkerSnapshot { snapshot: stale }).unwrap();
    let worker = db.read(|c| worker_task(c, &task)).unwrap().unwrap();
    assert_eq!(worker.status.state, AttemptState::Working, "an event after the snapshot wins");
    assert!(apply(event(10, "a1", state(AttemptState::Delivered))));

    let close = CloseWorkerTask {
        request_id: "close".into(),
        task_id: task.clone(),
        outcome: CloseOutcome::Adopted,
        reason: "合并了".into(),
    };
    db.execute(&Caller::User, &close).unwrap();
    assert_eq!(db.read(|c| load_task(c, &task)).unwrap().phase, Phase::Done);
}

/// A stop takes one reply. In the first real acceptance the user and Angela continued a stopped
/// worker at once, and the later message went into its next turn (worker-interface.md §3).
#[test]
fn a_second_reply_to_a_stop_is_refused_saying_who_replied() {
    let db = db();
    let task = queued_task(&db);
    hand_over(&db, &task);
    let blocked = Blocked { need: Need::Resume, question_id: None, stop_id: Some("turn_1".into()) };
    let stopped =
        EventBody::StateChanged { state: AttemptState::Blocked, blocked: Some(blocked), reason: Some("中断".into()) };
    db.execute(&Caller::Runtime, &ApplyWorkerEvent { task_id: task.clone(), event: event(1, "a1", stopped) }).unwrap();

    let user = SendMessage {
        request_id: "user".into(),
        role: WORKER.into(),
        task_id: Some(task.clone()),
        body: "继续".into(),
        reply_to: None,
        stop_id: Some("turn_1".into()),
        question_id: None,
    };
    db.execute(&Caller::User, &user).unwrap();
    let angela = SendToRole {
        request_id: "angela".into(),
        role: WORKER.into(),
        task_id: Some(task.clone()),
        quotes: Vec::new(),
        context: Vec::new(),
        ledger: Vec::new(),
        interpretation: "接着做".into(),
        stop_id: Some("turn_1".into()),
    };
    let refused = db.execute(&Caller::Manager { turn_id: "angela-turn".into() }, &angela).unwrap_err();
    let Error::Rejected { code, message } = refused else { panic!("{refused:?}") };
    assert_eq!(code, "stop_answered");
    assert!(message.contains("用户"), "it says who replied: {message}");
}
