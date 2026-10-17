//! Harness processes do not see the gh credential variables of the backend (#10;
//! harness-adapter.md §1.6), and get their role's own variables only (tech-plan.md §4.5). The
//! worker hosts' agents get the same trimming from the same code (`lobotomy-agent`). In a file of its own: the test changes the process environment.

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use lobotomy_core::Caller;
use lobotomy_core::id::now_ms;
use lobotomy_core::task::SendMessage;
use lobotomy_core::turn::{SetRoleHarness, TurnState, last_turn};
use serde_json::Value;

// The contract is independent of the production filter, so omitting a name there fails here.
const GH_CREDENTIAL_VARS: &[&str] = &["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];

fn seen_env(cwd: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(diag(cwd).join("last-env.json")).unwrap()).unwrap()
}

#[tokio::test]
async fn harness_processes_do_not_see_gh_tokens() {
    for name in GH_CREDENTIAL_VARS {
        // SAFETY: the only test in this binary, set before any thread reads the environment.
        unsafe { std::env::set_var(name, "dummy") };
    }
    let dir = tempfile::tempdir().unwrap();
    // A role's own variables reach its turns only (tech-plan.md §4.5).
    std::fs::create_dir_all(host_dir(dir.path())).unwrap();
    let vars = "[Angela]\nANTHROPIC_BASE_URL = \"https://example.test/anthropic\"\n[w]\nANTHROPIC_AUTH_TOKEN = \"not hers to see\"\n";
    std::fs::write(host_dir(dir.path()).join("role-env.toml"), vars).unwrap();
    let backend = start(dir.path()).await;

    // Angela's turn, on the fake Codex, which writes down the environment it sees.
    let angela = backend.backend.angela().expect("Angela opened");
    let codex = SetRoleHarness { request_id: "h1".into(), role: "Angela".into(), harness: "codex".into() };
    angela.db.execute(&Caller::User, &codex).unwrap();
    let message = SendMessage {
        request_id: "m1".into(),
        role: "Angela".into(),
        task_id: None,
        body: "anything".into(),
        reply_to: None,
        stop_id: None,
    };
    angela.db.execute(&Caller::User, &message).unwrap();
    angela.wake.notify_one();
    wait_for("her turn to end", || {
        angela.db.read(|c| last_turn(c, "Angela")).unwrap().filter(|t| t.state == TurnState::Ended)
    })
    .await;
    let env = seen_env(&host_dir(dir.path()).join("angela").join("work"));
    for name in GH_CREDENTIAL_VARS {
        assert_eq!(env[name], Value::Null, "{name} reached the turn");
    }
    assert_eq!(env["GH_CONFIG_DIR"], angela.empty_gh_config_dir().to_string_lossy().as_ref());
    assert_eq!(env["CLAUDE_CODE_DISABLE_AUTO_MEMORY"], "1", "memory is Lobotomy's, not the harness's");
    assert_eq!(env["CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"], "1", "background work would end with the turn");
    assert_eq!(env["ANTHROPIC_BASE_URL"], "https://example.test/anthropic");
    assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], Value::Null, "another role's variables stay its own");

    // A quota check.
    let host = backend.project.host.clone();
    host.db.block("codex", None, "test", now_ms()).unwrap();
    host.retry("codex").await.unwrap();
    let env = seen_env(&host_dir(dir.path()).join("probe"));
    for name in GH_CREDENTIAL_VARS {
        assert_eq!(env[name], Value::Null, "{name} reached the quota check");
    }
    assert_eq!(env["GH_CONFIG_DIR"], host_dir(dir.path()).join("gh-empty").to_string_lossy().as_ref());
    backend.shutdown(Duration::from_secs(5)).await;
}
