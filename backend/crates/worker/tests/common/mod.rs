//! The orchestrator's side of a worker host process, for the tests: the protocol on its stdin and
//! stdout, with the fake Codex.
#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use lobotomy_worker_protocol::event::{Artifact, AttemptState, Event, EventBody};
use lobotomy_worker_protocol::ops::{
    Action, Budget, CreateTaskParams, Hello, HelloParams, SendMessageParams, Snapshot, Subscribe, SubscribeParams,
    TaskSpec,
};
use lobotomy_worker_protocol::rpc::{self, Id, Message, Request, RpcError};
use lobotomy_worker_protocol::{Method, PROTOCOL_VERSION};
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub const WAIT: Duration = Duration::from_secs(60);

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../lobotomyd/tests/fixtures").join(name);
    serde_json::to_string(&["node".to_owned(), path.to_string_lossy().into_owned()]).unwrap()
}

pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    events: VecDeque<Event>,
}

impl Worker {
    pub async fn start(root: &Path) -> Self {
        Self::start_with(root, &[], &[]).await
    }

    /// With `args` after the required ones, and `env` added to the process's.
    pub async fn start_with(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lobotomy-worker"))
            .args(["--root".as_ref(), root.as_os_str(), "--harness".as_ref(), "codex".as_ref()])
            .args(args)
            .env("LOBOTOMY_CODEX", fixture("fake-codex.mjs"))
            .env("LOBOTOMY_CLAUDE", fixture("fake-claude.mjs"))
            .envs(env.iter().copied())
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

    pub async fn call<M: Method>(&mut self, params: &M::Params) -> Result<M::Result, RpcError> {
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
    pub async fn event(&mut self, what: &str, wanted: impl Fn(&Event) -> bool) -> Event {
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

    pub async fn state(&mut self, task: &str, state: AttemptState) -> Event {
        let what = format!("{task} to be {state:?}");
        self.event(&what, |e| {
            e.task_id == task && matches!(&e.body, EventBody::StateChanged { state: s, .. } if *s == state)
        })
        .await
    }

    /// The next version of an artifact whose id ends with `suffix`.
    pub async fn artifact(&mut self, suffix: &str) -> Artifact {
        let what = format!("an artifact {suffix}");
        let event = self.event(&what, |e| published(e).is_some_and(|a| a.artifact_id.ends_with(suffix))).await;
        published(&event).unwrap().clone()
    }

    pub async fn subscribe(&mut self, after: BTreeMap<String, u64>) -> Snapshot {
        self.call::<Subscribe>(&SubscribeParams { after }).await.unwrap()
    }

    /// Kills the process, as a crash would.
    pub async fn kill(mut self) {
        self.child.kill().await.unwrap();
    }

    /// Closes stdin, as the orchestrator does when it quits, and waits for the process to end.
    pub async fn quit(mut self) {
        drop(self.stdin);
        let status = tokio::time::timeout(WAIT, self.child.wait()).await.unwrap().unwrap();
        assert!(status.success(), "{status}");
    }
}

pub fn published(event: &Event) -> Option<&Artifact> {
    match &event.body {
        EventBody::ArtifactPublished { artifact } => Some(artifact),
        _ => None,
    }
}

pub fn spec(words: &str) -> TaskSpec {
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

pub fn create(task: &str, words: &str) -> CreateTaskParams {
    CreateTaskParams { task_id: task.into(), spec_version: 1, spec: spec(words) }
}

pub fn message(task: &str, id: &str, body: &str) -> SendMessageParams {
    SendMessageParams {
        task_id: task.into(),
        message_id: id.into(),
        body: body.into(),
        refs: Vec::new(),
        question_id: None,
    }
}

pub fn root(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("worker")
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The upstream repository, bare, as a forge holds it: one commit on `main`.
pub fn upstream(dir: &Path) -> PathBuf {
    let work = dir.join("upstream-work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "--quiet", "--initial-branch", "main"]);
    git(&work, &["config", "user.name", "Test User"]);
    git(&work, &["config", "user.email", "user@example.com"]);
    std::fs::write(work.join("README.md"), "# project\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "--quiet", "-m", "initial"]);
    git(dir, &["clone", "--quiet", "--bare", "upstream-work", "upstream.git"]);
    dir.join("upstream.git")
}
