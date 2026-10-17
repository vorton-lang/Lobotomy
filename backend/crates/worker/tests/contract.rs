//! The worker host as the orchestrator sees it: the protocol on its stdin and stdout, with the fake
//! Codex (worker-interface.md §3–§5, §9).

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use lobotomy_worker_protocol::event::{AttemptState, Event, EventBody, Need, Receipt};
use lobotomy_worker_protocol::ops::{
    Action, Budget, Close, CloseOutcome, CloseParams, CreateTask, CreateTaskParams, Hello, HelloParams, Revise,
    SendMessage, SendMessageParams, Snapshot, Stop, StopLevel, StopParams, Subscribe, SubscribeParams, TaskSpec,
};
use lobotomy_worker_protocol::rpc::{self, Id, Message, Request, RpcError};
use lobotomy_worker_protocol::{Method, PROTOCOL_VERSION};
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

const WAIT: Duration = Duration::from_secs(60);

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../lobotomyd/tests/fixtures").join(name);
    serde_json::to_string(&["node".to_owned(), path.to_string_lossy().into_owned()]).unwrap()
}

/// The orchestrator's side of one worker host process.
struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    events: VecDeque<Event>,
}

impl Worker {
    async fn start(root: &Path) -> Self {
        Self::start_with(root, &[]).await
    }

    async fn start_with(root: &Path, extra: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lobotomy-worker"))
            .args(["--root".as_ref(), root.as_os_str(), "--harness".as_ref(), "codex".as_ref()])
            .args(extra)
            .env("LOBOTOMY_CODEX", fixture("fake-codex.mjs"))
            .env("LOBOTOMY_CLAUDE", fixture("fake-claude.mjs"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut worker = Self { child, stdin, stdout, next_id: 0, events: VecDeque::new() };
        let hello = worker.call::<Hello>(&HelloParams { protocols: vec![PROTOCOL_VERSION.into()] }).await.unwrap();
        assert_eq!(hello.capabilities.agent.harness, "codex");
        worker
    }

    async fn call<M: Method>(&mut self, params: &M::Params) -> Result<M::Result, RpcError> {
        self.next_id += 1;
        let id = Id::Number(self.next_id);
        rpc::write(&mut self.stdin, &Message::Request(Request::new::<M>(id.clone(), params))).await.unwrap();
        loop {
            match self.read().await {
                Message::Response(response) if response.id == id => return response.into_result::<M>(),
                Message::Response(other) => panic!("a response to another request: {other:?}"),
                Message::Notification(_) | Message::Request(_) => {}
            }
        }
    }

    /// The next message; events go to the queue on the way.
    async fn read(&mut self) -> Message {
        let message = tokio::time::timeout(WAIT, rpc::read(&mut self.stdout))
            .await
            .expect("the worker host said nothing for a minute")
            .unwrap()
            .expect("the worker host closed its stdout");
        if let Message::Notification(n) = &message
            && n.method == "event"
        {
            self.events.push_back(serde_json::from_value(n.params.clone()).unwrap());
        }
        message
    }

    /// The first event, from the queue or later, that `wanted` accepts; the ones before it are
    /// consumed.
    async fn event(&mut self, what: &str, wanted: impl Fn(&Event) -> bool) -> Event {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            while let Some(event) = self.events.pop_front() {
                if wanted(&event) {
                    return event;
                }
            }
            assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
            self.read().await;
        }
    }

    async fn state(&mut self, task: &str, state: AttemptState) -> Event {
        let what = format!("{task} to be {state:?}");
        self.event(&what, |e| {
            e.task_id == task && matches!(&e.body, EventBody::StateChanged { state: s, .. } if *s == state)
        })
        .await
    }

    async fn subscribe(&mut self, after: BTreeMap<String, u64>) -> Snapshot {
        self.call::<Subscribe>(&SubscribeParams { after }).await.unwrap()
    }

    /// Kills the process, as a crash would.
    async fn kill(mut self) {
        self.child.kill().await.unwrap();
    }

    /// Closes stdin, as the orchestrator does when it quits, and waits for the process to end.
    async fn quit(mut self) {
        drop(self.stdin);
        let status = tokio::time::timeout(WAIT, self.child.wait()).await.unwrap().unwrap();
        assert!(status.success(), "{status}");
    }
}

fn spec(words: &str) -> TaskSpec {
    TaskSpec {
        title: "写 work.txt".into(),
        user_words: words.into(),
        interpretation: "照原话做".into(),
        actions: vec![Action::EditCode],
        done_when: "work.txt 存在".into(),
        required_checks: Vec::new(),
        stop_when: "做完就停".into(),
        budget: Budget::default(),
        ask_when: "不确定时".into(),
    }
}

fn create(task: &str, words: &str) -> CreateTaskParams {
    CreateTaskParams { task_id: task.into(), spec_version: 1, spec: spec(words) }
}

fn message(task: &str, id: &str, body: &str) -> SendMessageParams {
    SendMessageParams {
        task_id: task.into(),
        message_id: id.into(),
        body: body.into(),
        refs: Vec::new(),
        question_id: None,
    }
}

fn root(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("worker")
}

#[tokio::test]
async fn a_task_is_delivered_reworked_and_closed_and_retries_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut worker = Worker::start(&root(&dir)).await;
    assert!(worker.subscribe(BTreeMap::new()).await.tasks.is_empty());

    let attempt = worker.call::<CreateTask>(&create("t", "FAKE:done")).await.unwrap().attempt_id;
    let again = worker.call::<CreateTask>(&create("t", "FAKE:done")).await.unwrap().attempt_id;
    assert_eq!(again, attempt, "a retry returns the first result");
    let reused = worker.call::<CreateTask>(&create("t", "something else")).await.unwrap_err();
    assert_eq!(reused.reason(), Some("idempotency_key_reused"));
    worker.state("t", AttemptState::Working).await;
    worker.state("t", AttemptState::Delivered).await;

    // Review rework goes on in the same attempt (§0 rule 4).
    let rework = message("t", "m1", "FAKE:done FAKE:text=again");
    assert_eq!(worker.call::<SendMessage>(&rework).await.unwrap().attempt_id, attempt);
    assert_eq!(worker.call::<SendMessage>(&rework).await.unwrap().attempt_id, attempt);
    worker
        .event("m1 to be delivered", |e| {
            matches!(&e.body, EventBody::MessageReceipt { message_id, receipt: Receipt::Delivered } if message_id == "m1")
        })
        .await;
    let delivered = worker.state("t", AttemptState::Delivered).await;
    assert_eq!(delivered.attempt_id, attempt);

    worker.call::<Close>(&CloseParams { task_id: "t".into(), outcome: CloseOutcome::Abandoned }).await.unwrap();
    worker.state("t", AttemptState::Abandoned).await;
    let late = worker.call::<SendMessage>(&message("t", "m2", "more")).await.unwrap_err();
    assert_eq!(late.reason(), Some("attempt_ended"));
    let snapshot = worker.subscribe(BTreeMap::new()).await;
    assert_eq!(snapshot.tasks[0].closed, Some(CloseOutcome::Abandoned));
    worker.quit().await;
}

#[tokio::test]
async fn a_question_blocks_the_attempt_until_a_message_answers_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut worker = Worker::start(&root(&dir)).await;
    worker.subscribe(BTreeMap::new()).await;
    worker.call::<CreateTask>(&create("t", "FAKE:blocked")).await.unwrap();
    let blocked = worker.state("t", AttemptState::Blocked).await;
    let EventBody::StateChanged { blocked: Some(why), .. } = blocked.body else { panic!("{blocked:?}") };
    assert_eq!(why.need, Need::Input);
    let question = why.question_id.expect("needs_input names its question");

    let answer =
        SendMessageParams { question_id: Some(question.clone()), ..message("t", "a", "用默认值 FAKE:done") };
    worker.call::<SendMessage>(&answer).await.unwrap();
    worker
        .event(
            "the question to close",
            |e| matches!(&e.body, EventBody::QuestionClosed { question_id, .. } if *question_id == question),
        )
        .await;
    worker.state("t", AttemptState::Delivered).await;
    worker.quit().await;
}

#[tokio::test]
async fn stops_take_effect_when_the_agent_has_exited_and_a_message_resumes_an_interrupted_turn() {
    let dir = tempfile::tempdir().unwrap();
    let mut worker = Worker::start(&root(&dir)).await;
    worker.subscribe(BTreeMap::new()).await;
    let attempt = worker.call::<CreateTask>(&create("t", "FAKE:sleep")).await.unwrap().attempt_id;
    worker.state("t", AttemptState::Working).await;
    // The CLI must have started before it can be interrupted.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let stopped = worker.call::<Stop>(&StopParams { task_id: "t".into(), level: StopLevel::Turn }).await.unwrap();
    assert_eq!(stopped.attempt_id, attempt);
    let held = worker.state("t", AttemptState::Blocked).await;
    let EventBody::StateChanged { blocked: Some(why), .. } = held.body else { panic!("{held:?}") };
    assert_eq!(why.need, Need::Resume);

    // The same attempt goes on (worker-interface.md §3, 语义更新 2026-10-17).
    assert_eq!(worker.call::<SendMessage>(&message("t", "go", "FAKE:sleep")).await.unwrap().attempt_id, attempt);
    worker.state("t", AttemptState::Working).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    worker.call::<Stop>(&StopParams { task_id: "t".into(), level: StopLevel::Attempt }).await.unwrap();
    let canceled = worker.state("t", AttemptState::Canceled).await;
    assert_eq!(canceled.attempt_id, attempt);
    let again = worker.call::<Stop>(&StopParams { task_id: "t".into(), level: StopLevel::Attempt }).await.unwrap();
    assert_eq!(again.state, AttemptState::Canceled, "stopping a final attempt changes nothing");
    worker.quit().await;
}

#[tokio::test]
async fn a_revision_supersedes_the_queued_messages_and_starts_a_new_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let mut worker = Worker::start(&root(&dir)).await;
    worker.subscribe(BTreeMap::new()).await;
    let first = worker.call::<CreateTask>(&create("t", "FAKE:sleep")).await.unwrap().attempt_id;
    worker.state("t", AttemptState::Working).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    worker.call::<SendMessage>(&message("t", "queued", "while it sleeps")).await.unwrap();

    let revise = CreateTaskParams { task_id: "t".into(), spec_version: 2, spec: spec("FAKE:done") };
    let revised = worker.call::<Revise>(&revise).await.unwrap();
    assert_ne!(revised.attempt_id, first);
    assert_eq!(revised.superseded, vec!["queued".to_owned()]);
    assert_eq!(worker.call::<Revise>(&revise).await.unwrap(), revised, "a retry returns the first result");

    let old = worker.state("t", AttemptState::Canceled).await;
    assert_eq!(old.attempt_id, first);
    let delivered = worker.state("t", AttemptState::Delivered).await;
    assert_eq!(delivered.attempt_id, revised.attempt_id);
    worker.quit().await;
}

#[tokio::test]
async fn after_a_crash_the_snapshot_says_where_the_task_stands_and_later_events_are_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let mut worker = Worker::start(&root(&dir)).await;
    worker.subscribe(BTreeMap::new()).await;
    worker.call::<CreateTask>(&create("t", "FAKE:sleep")).await.unwrap();
    let working = worker.state("t", AttemptState::Working).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    worker.kill().await;

    // An orchestrator that missed `working` gets it again, from the store, after the snapshot.
    let mut worker = Worker::start(&root(&dir)).await;
    let snapshot = worker.subscribe(BTreeMap::from([("t".to_owned(), working.seq - 1)])).await;
    assert_eq!(snapshot.tasks.len(), 1);
    let replayed = worker.event("the replay", |_| true).await;
    assert_eq!((replayed.seq, &replayed.body), (working.seq, &working.body));
    // The CLI ended with the worker host; its turn is reconciled as interrupted.
    let held = worker.state("t", AttemptState::Blocked).await;
    let EventBody::StateChanged { blocked: Some(why), .. } = held.body else { panic!("{held:?}") };
    assert_eq!(why.need, Need::Resume);
    worker.quit().await;
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The upstream repository: one commit on `main`.
fn upstream(dir: &Path) -> PathBuf {
    let repo = dir.join("upstream");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch", "main"]);
    git(&repo, &["config", "user.name", "Test User"]);
    git(&repo, &["config", "user.email", "user@example.com"]);
    std::fs::write(repo.join("README.md"), "# project\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "initial"]);
    repo
}

fn published(event: &Event) -> Option<&lobotomy_worker_protocol::event::Artifact> {
    match &event.body {
        EventBody::ArtifactPublished { artifact } => Some(artifact),
        _ => None,
    }
}

/// The agent works in its own clone with L1's variables and cannot push; once its processes have
/// exited, the environment takes its work back into the mirror (worker-interface.md §6, §7).
#[tokio::test]
async fn a_delivery_is_taken_back_into_the_mirror() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = upstream(dir.path());
    let root = root(&dir);
    let mut worker = Worker::start_with(&root, &["--upstream", upstream.to_str().unwrap()]).await;
    worker.subscribe(BTreeMap::new()).await;
    let attempt = worker.call::<CreateTask>(&create("t", "FAKE:done")).await.unwrap().attempt_id;
    worker.state("t", AttemptState::Preparing).await;
    let change = worker.event("the change", |e| published(e).is_some_and(|a| a.artifact_id.ends_with(":change"))).await;
    let change = published(&change).unwrap().clone();
    let report = worker.event("the report", |e| published(e).is_some_and(|a| a.artifact_id.ends_with(":report"))).await;
    assert!(published(&report).unwrap().text.as_deref().unwrap().contains("写了 work.txt"));
    worker.state("t", AttemptState::Delivered).await;

    // The fake agent left work.txt uncommitted; the environment committed it.
    assert_eq!(change.size.unwrap().files, 1);
    assert!(change.title.contains("环境提交"), "{}", change.title);
    let mirror = root.join("mirror.git");
    assert_eq!(git(&mirror, &["rev-parse", "refs/lobotomy/t-1"]), change.version);
    assert_eq!(git(&mirror, &["show", &format!("{}:work.txt", change.version)]), "hi");

    let clone = root.join("tasks").join("t").join("a1");
    assert_eq!(git(&clone, &["branch", "--show-current"]), "lobotomy/t-1");
    let env: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(clone.join(".git").join("last-env.json")).unwrap()).unwrap();
    assert_eq!(env["GIT_CONFIG_NOSYSTEM"], "1");
    assert_eq!(env["LOBOTOMY_PORT_BASE"], "42000");
    assert!(env["TMP"].as_str().unwrap().ends_with("a1.tmp"), "{env}");
    // With the agent's git configuration, a push from the clone goes nowhere.
    let push = std::process::Command::new("git")
        .arg("-C")
        .arg(&clone)
        .args(["push", "origin", "HEAD"])
        .env("GIT_CONFIG_GLOBAL", env["GIT_CONFIG_GLOBAL"].as_str().unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(!push.status.success(), "the agent pushed to the mirror");

    // Review rework: a new version of the same change.
    worker.call::<SendMessage>(&message("t", "m1", "FAKE:done FAKE:text=again")).await.unwrap();
    let next =
        worker.event("the next version", |e| published(e).is_some_and(|a| a.artifact_id.ends_with(":change"))).await;
    let next = published(&next).unwrap();
    assert_eq!(next.artifact_id, format!("{attempt}:change"));
    assert_ne!(next.version, change.version);
    assert_eq!(git(&mirror, &["show", &format!("{}:work.txt", next.version)]), "again");
    worker.state("t", AttemptState::Delivered).await;
    let snapshot = worker.subscribe(BTreeMap::new()).await;
    let versions: Vec<&str> = snapshot.tasks[0].artifacts.iter().map(|a| a.version.as_str()).collect();
    assert!(versions.contains(&next.version.as_str()), "{versions:?}");
    worker.quit().await;
}
