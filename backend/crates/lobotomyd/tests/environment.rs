//! Harness processes do not see the gh credential variables of the backend (#10;
//! harness-adapter.md §1.6). In a file of its own: the test changes the process environment.

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use lobotomy_core::id::now_ms;
use lobotomyd::capability::REMOVED_VARS;
use serde_json::Value;

fn seen_env(cwd: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(diag(cwd).join("last-env.json")).unwrap()).unwrap()
}

#[tokio::test]
async fn harness_processes_do_not_see_gh_tokens() {
    for name in REMOVED_VARS {
        // SAFETY: the only test in this binary, set before any thread reads the environment.
        unsafe { std::env::set_var(name, "dummy") };
    }
    let dir = tempfile::tempdir().unwrap();
    let backend = start(dir.path()).await;

    // A role turn.
    create_task(&backend.project.db, "anything");
    ended_turn(&backend.project.db).await;
    let env = seen_env(&slot(dir.path()));
    for name in REMOVED_VARS {
        assert_eq!(env[name], Value::Null, "{name} reached the turn");
    }
    assert_eq!(env["GH_CONFIG_DIR"], backend.project.empty_gh_config_dir().to_string_lossy().as_ref());

    // A quota check.
    let host = backend.project.host.clone();
    host.db.block("codex", None, "test", now_ms()).unwrap();
    host.retry("codex").await.unwrap();
    let env = seen_env(&host_dir(dir.path()).join("probe"));
    for name in REMOVED_VARS {
        assert_eq!(env[name], Value::Null, "{name} reached the quota check");
    }
    assert_eq!(env["GH_CONFIG_DIR"], host_dir(dir.path()).join("gh-empty").to_string_lossy().as_ref());
    backend.shutdown(Duration::from_secs(5)).await;
}
