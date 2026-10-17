//! The worker host on a forge, against a fake GitHub (worker-interface.md §6, §7): the change goes
//! out as a pull request from the mirror, CI failures go back to the agent, and adopting merges
//! the pull request only at the version asked for.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, patch, put};
use axum::{Json, Router};
use common::*;
use lobotomy_worker_protocol::event::{AttemptState, EventBody, EvidenceStatus};
use lobotomy_worker_protocol::ops::{
    Action, Adopt, AdoptOutcome, AdoptParams, AdoptRejection, Close, CloseOutcome, CloseParams, CreateTask,
    CreateTaskParams,
};
use serde_json::{Value, json};

/// What the fake CI's job log ends with: the fake agent's next turn reads it and fixes the change.
const LOG: &str = "error: work.txt is wrong\nFAKE:done FAKE:text=fixed\n";

#[derive(Clone)]
struct Fake {
    /// The repository the worker host pushes to, as GitHub would hold it.
    upstream: PathBuf,
    /// By number: the head branch, and whether it is open.
    pulls: Arc<Mutex<Vec<(String, bool)>>>,
}

fn pull(number: usize) -> Value {
    json!({ "number": number, "html_url": format!("https://example.test/o/r/pull/{number}") })
}

async fn list(State(fake): State<Fake>, Query(query): Query<HashMap<String, String>>) -> Json<Value> {
    let head = query.get("head").and_then(|h| h.split_once(':')).map(|(_, branch)| branch.to_owned());
    let pulls = fake.pulls.lock().unwrap();
    let open = pulls.iter().enumerate().filter(|(_, (b, open))| *open && Some(b) == head.as_ref());
    Json(json!(open.map(|(i, _)| pull(i + 1)).collect::<Vec<_>>()))
}

async fn create(State(fake): State<Fake>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    let mut pulls = fake.pulls.lock().unwrap();
    pulls.push((body["head"].as_str().unwrap().to_owned(), true));
    (StatusCode::CREATED, Json(pull(pulls.len())))
}

async fn update(State(fake): State<Fake>, Path((_, _, n)): Path<(String, String, usize)>, Json(body): Json<Value>) {
    if body["state"] == "closed" {
        fake.pulls.lock().unwrap()[n - 1].1 = false;
    }
}

/// `ci-ok` passes once work.txt says "fixed".
async fn checks(State(fake): State<Fake>, Path((_, _, sha)): Path<(String, String, String)>) -> Json<Value> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&fake.upstream)
        .args(["show", &format!("{sha}:work.txt")])
        .output()
        .unwrap();
    let conclusion = if String::from_utf8_lossy(&out.stdout).trim() == "fixed" { "success" } else { "failure" };
    Json(json!({ "check_runs": [{
        "id": 7, "name": "ci-ok", "status": "completed", "conclusion": conclusion,
        "html_url": "https://example.test/runs/7",
    }] }))
}

/// Merges only if the pull request's head is still the commit asked for.
async fn merge(
    State(fake): State<Fake>,
    Path((_, _, n)): Path<(String, String, usize)>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let branch = fake.pulls.lock().unwrap()[n - 1].0.clone();
    let head = git(&fake.upstream, &["rev-parse", &format!("refs/heads/{branch}")]);
    if body["sha"] != head.as_str() {
        return (StatusCode::CONFLICT, Json(json!({ "message": "Head branch was modified" })));
    }
    git(&fake.upstream, &["update-ref", "refs/heads/main", &head]);
    fake.pulls.lock().unwrap()[n - 1].1 = false;
    (StatusCode::OK, Json(json!({ "merged": true })))
}

async fn delete_branch(State(fake): State<Fake>, Path((_, _, branch)): Path<(String, String, String)>) -> StatusCode {
    git(&fake.upstream, &["update-ref", "-d", &format!("refs/heads/{branch}")]);
    StatusCode::NO_CONTENT
}

async fn fake_github(upstream: PathBuf) -> String {
    let fake = Fake { upstream, pulls: Arc::default() };
    let app = Router::new()
        .route("/repos/{o}/{r}/pulls", get(list).post(create))
        .route("/repos/{o}/{r}/pulls/{n}", patch(update))
        .route("/repos/{o}/{r}/pulls/{n}/merge", put(merge))
        .route("/repos/{o}/{r}/commits/{sha}/check-runs", get(checks))
        .route("/repos/{o}/{r}/actions/jobs/{id}/logs", get(|| async { LOG }))
        .route("/repos/{o}/{r}/git/refs/heads/{*branch}", delete(delete_branch))
        .with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

fn evidence(status: EvidenceStatus) -> impl Fn(&lobotomy_worker_protocol::event::Event) -> bool {
    move |e| matches!(&e.body, EventBody::EvidenceUpdated { evidence } if evidence.name == "ci-ok" && evidence.status == status)
}

#[tokio::test]
async fn a_change_goes_out_as_a_pull_request_ci_failures_go_back_and_adopting_merges_it() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = upstream(dir.path());
    let api = fake_github(upstream.clone()).await;
    let root = root(&dir);
    let args = ["--upstream", upstream.to_str().unwrap(), "--forge", "github", "--repo", "o/r", "--forge-api", &api];
    let mut worker = Worker::start_with(&root, &args, &[("LOBOTOMY_FORGE_TOKEN", "secret-token")]).await;
    worker.subscribe(BTreeMap::new()).await;
    let mut spec = spec("FAKE:done");
    spec.actions.push(Action::OpenPullRequest);
    spec.required_checks = vec!["ci-ok".into()];
    spec.budget.ci_rounds = Some(2);
    let params = CreateTaskParams { task_id: "t".into(), spec_version: 1, spec };
    worker.call::<CreateTask>(&params).await.unwrap();

    let first = worker.artifact(":change").await;
    assert_eq!(first.url.as_deref(), Some("https://example.test/o/r/pull/1"));
    // The failure and its log went back to the agent, which fixed the change on the same branch.
    worker.event("ci-ok to fail", evidence(EvidenceStatus::Failed)).await;
    let fixed = worker.artifact(":change").await;
    assert_ne!(fixed.version, first.version);
    assert_eq!(fixed.url, first.url, "the same pull request");
    worker.event("ci-ok to pass", evidence(EvidenceStatus::Passed)).await;
    worker.state("t", AttemptState::Delivered).await;

    // The agent never had the forge's token.
    let clone = root.join("tasks").join("t").join("a1");
    let env: Value =
        serde_json::from_str(&std::fs::read_to_string(clone.join(".git").join("last-env.json")).unwrap()).unwrap();
    assert_eq!(env["LOBOTOMY_FORGE_TOKEN"], Value::Null);

    // Adopting is pinned to the version.
    let adopt = |version: &str| AdoptParams {
        task_id: "t".into(),
        artifact_id: fixed.artifact_id.clone(),
        version: version.into(),
    };
    let stale = worker.call::<Adopt>(&adopt(&first.version)).await.unwrap();
    assert!(matches!(stale, AdoptOutcome::Rejected { reason: AdoptRejection::VersionChanged, .. }), "{stale:?}");
    assert_eq!(worker.call::<Adopt>(&adopt(&fixed.version)).await.unwrap(), AdoptOutcome::Adopted);
    assert_eq!(worker.call::<Adopt>(&adopt(&fixed.version)).await.unwrap(), AdoptOutcome::Adopted, "idempotent");
    worker.state("t", AttemptState::Adopted).await;
    assert_eq!(git(&upstream, &["rev-parse", "refs/heads/main"]), fixed.version);
    worker.call::<Close>(&CloseParams { task_id: "t".into(), outcome: CloseOutcome::Adopted }).await.unwrap();
    // A few rounds of the task's loop later, it is still adopted, not abandoned.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let snapshot = worker.subscribe(BTreeMap::new()).await;
    assert_eq!(
        (snapshot.tasks[0].state, snapshot.tasks[0].closed),
        (AttemptState::Adopted, Some(CloseOutcome::Adopted))
    );
    worker.quit().await;
}
