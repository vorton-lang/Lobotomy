//! The wire format other implementations see (worker-interface.md §5, §10).

use lobotomy_worker_protocol::event::{AttemptState, Blocked, Event, EventBody, Need, Receipt};
use lobotomy_worker_protocol::ops::{AdoptOutcome, AdoptRejection, AttemptRef, CreateTask, Hello, HelloParams};
use lobotomy_worker_protocol::rpc::{self, Id, Message, Notification, Request, Response, RpcError};
use serde_json::json;

#[test]
fn an_event_is_flat_with_its_kind() {
    let event = Event {
        task_id: "t".into(),
        attempt_id: "a".into(),
        seq: 3,
        body: EventBody::StateChanged {
            state: AttemptState::Blocked,
            blocked: Some(Blocked { need: Need::Input, question_id: Some("q".into()) }),
            reason: None,
        },
    };
    let wire = serde_json::to_value(&event).unwrap();
    assert_eq!(
        wire,
        json!({
            "task_id": "t", "attempt_id": "a", "seq": 3, "kind": "state_changed", "state": "blocked",
            "blocked": { "need": "needs_input", "question_id": "q" }, "reason": null,
        })
    );
    assert_eq!(serde_json::from_value::<Event>(wire).unwrap(), event);

    let receipt = EventBody::MessageReceipt { message_id: "m".into(), receipt: Receipt::Bound { turn: "u".into() } };
    assert_eq!(
        serde_json::to_value(&receipt).unwrap(),
        json!({ "kind": "message_receipt", "message_id": "m", "receipt": { "status": "bound", "turn": "u" } })
    );
    let refused = AdoptOutcome::Rejected { reason: AdoptRejection::VersionChanged, detail: "head moved".into() };
    assert_eq!(
        serde_json::to_value(&refused).unwrap(),
        json!({ "outcome": "rejected", "reason": "version_changed", "detail": "head moved" })
    );
}

/// What a newer version adds is ignored, not refused (§8).
#[test]
fn unknown_fields_are_ignored() {
    let wire = json!({ "task_id": "t", "attempt_id": "a", "seq": 1, "kind": "progress", "line": "x", "later": 1 });
    let event: Event = serde_json::from_value(wire).unwrap();
    assert_eq!(event.body, EventBody::Progress { line: "x".into() });
}

#[test]
fn messages_are_told_apart_by_their_fields() {
    let request = Request::new::<Hello>(Id::Number(1), &HelloParams { protocols: vec!["0.1".into()] });
    let response = Response::ok::<CreateTask>(Id::String("c".into()), &AttemptRef { attempt_id: "a".into() });
    let notification = Notification::new("event", &json!({ "seq": 1 }));
    for message in [Message::Request(request), Message::Response(response), Message::Notification(notification)] {
        let text = serde_json::to_string(&message).unwrap();
        assert_eq!(serde_json::from_str::<Message>(&text).unwrap(), message, "{text}");
    }

    let refused = Response::err(Id::Number(2), RpcError::rejected("unknown_task", "no task t"));
    let error = refused.into_result::<CreateTask>().unwrap_err();
    assert_eq!(error.reason(), Some("unknown_task"));
}

#[tokio::test]
async fn a_bad_line_does_not_end_the_stream() {
    let (mut ours, theirs) = tokio::io::duplex(4096);
    let note = Message::Notification(Notification::new("event", &json!({})));
    rpc::write(&mut ours, &note).await.unwrap();
    tokio::io::AsyncWriteExt::write_all(&mut ours, b"\nnot json\n").await.unwrap();
    rpc::write(&mut ours, &note).await.unwrap();
    drop(ours);

    let mut reader = tokio::io::BufReader::new(theirs);
    assert_eq!(rpc::read(&mut reader).await.unwrap(), Some(note.clone()));
    assert_eq!(rpc::read(&mut reader).await.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(rpc::read(&mut reader).await.unwrap(), Some(note));
    assert_eq!(rpc::read(&mut reader).await.unwrap(), None);
}
