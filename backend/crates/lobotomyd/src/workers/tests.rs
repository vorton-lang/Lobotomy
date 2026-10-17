use super::*;

use lobotomy_core::task::{CreateTask, EditCriteria, Phase};
use lobotomy_core::worker::worker_task;
use lobotomy_worker_protocol::event::{EventBody, TaskSnapshot};
use lobotomy_worker_protocol::ops::{AttemptRef, Revised, Snapshot};

/// A worker may emit its final state before replying to create/revise. Apply those early events
/// before releasing each RPC response, without relying on process scheduling or sleeps.
#[tokio::test]
async fn early_create_and_revise_events_are_reconciled_after_registration() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::open(&dir.path().join("host"), crate::host::HarnessConfig::detect().unwrap()).unwrap());
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let project = crate::projects::create(&host, &repo, Some(dir.path().join("project"))).await.unwrap();
    let name = host.db.project(&project.id).unwrap().unwrap().name;
    std::fs::write(
        host.dir.join("workers.toml"),
        format!("[worker.w]\nproject = {name:?}\nagent = {{ harness = \"codex\" }}\n"),
    )
    .unwrap();
    let projects = Arc::new(Projects::new(host.clone()));
    let workers = Workers::load(host, &projects).unwrap();
    workers.sync_roles(&project).await.unwrap();

    let mut echo = Command::new("node")
        .args(["-e", "process.stdin.pipe(process.stdout)"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = Arc::new(Client {
        stdin: tokio::sync::Mutex::new(echo.stdin.take()),
        pending: Mutex::new(HashMap::new()),
        next: AtomicI64::new(1),
    });
    workers.clients.lock().unwrap().insert("w".into(), client.clone());
    let mut requests = BufReader::new(echo.stdout.take().unwrap());
    let observed = project.clone();
    let fake = tokio::spawn(async move {
        let mut latest = None;
        let mut round = 0;
        while let Some(Message::Request(request)) = rpc::read(&mut requests).await.unwrap() {
            let response = if request.method == Subscribe::NAME {
                Response::ok::<Subscribe>(request.id.clone(), &Snapshot { tasks: vec![latest.clone().unwrap()] })
            } else {
                let params: CreateTaskParams = serde_json::from_value(request.params.clone()).unwrap();
                round += 1;
                let attempt_id = format!("a{round}");
                let event = Event {
                    task_id: params.task_id.clone(),
                    attempt_id: attempt_id.clone(),
                    seq: round,
                    body: EventBody::StateChanged { state: AttemptState::Delivered, blocked: None, reason: None },
                };
                let applied = runtime(&observed, ApplyWorkerEvent { task_id: params.task_id.clone(), event }).await;
                if round == 1 {
                    assert!(applied.is_err(), "the project does not know the worker task yet");
                } else {
                    assert!(applied.unwrap(), "the old attempt records the seq but not the new state");
                }
                latest = Some(TaskSnapshot {
                    task_id: params.task_id,
                    spec_version: params.spec_version,
                    attempt_id: attempt_id.clone(),
                    state: AttemptState::Delivered,
                    blocked: None,
                    reason: None,
                    closed: None,
                    question: None,
                    messages: Vec::new(),
                    artifacts: Vec::new(),
                    evidence: Vec::new(),
                    seq: round,
                });
                if request.method == ops::CreateTask::NAME {
                    Response::ok::<ops::CreateTask>(request.id.clone(), &AttemptRef { attempt_id })
                } else {
                    assert_eq!(request.method, ops::Revise::NAME);
                    Response::ok::<ops::Revise>(request.id.clone(), &Revised { attempt_id, superseded: Vec::new() })
                }
            };
            let Id::Number(id) = request.id else { panic!("numeric call id") };
            client.pending.lock().unwrap().remove(&id).unwrap().send(response).unwrap();
        }
    });

    let task_id = project
        .db
        .execute(
            &Caller::User,
            &CreateTask {
                request_id: "create".into(),
                title: "early delivery".into(),
                body: "read it".into(),
                criteria: "a report".into(),
                executor: "w".into(),
            },
        )
        .unwrap()
        .id;
    workers.dispatch(&project).await.unwrap();
    let first = project.db.read(|c| worker_task(c, &task_id)).unwrap().unwrap();
    assert_eq!(first.attempt_id, "a1");
    assert_eq!(first.status.state, AttemptState::Delivered);
    assert_eq!(project.db.read(|c| load_task(c, &task_id)).unwrap().phase, Phase::Accepting);

    project
        .db
        .execute(
            &Caller::User,
            &EditCriteria {
                request_id: "revise".into(),
                task_id: task_id.clone(),
                expected_version: 1,
                text: "a revised report".into(),
            },
        )
        .unwrap();
    workers.revise(&project, &first, &task_id).await.unwrap();
    let revised = project.db.read(|c| worker_task(c, &task_id)).unwrap().unwrap();
    assert_eq!(revised.attempt_id, "a2");
    assert_eq!(revised.spec_version, 2);
    assert_eq!(revised.status.state, AttemptState::Delivered);
    assert_eq!(project.db.read(|c| load_task(c, &task_id)).unwrap().phase, Phase::Accepting);
    fake.abort();
    echo.kill().await.unwrap();
}
