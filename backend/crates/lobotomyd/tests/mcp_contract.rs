//! Contract test: the official CLIs against Lobotomy's MCP service (m1-plan.md §3, increment 2).
//!
//! Each test serves `lobotomyd::mcp::router` with a probe tool, runs one real turn of a CLI with
//! the URL of a fresh token, and checks three things:
//! 1. the CLI calls the tool, and the tool handler reads the token from the URL;
//! 2. every HTTP exchange succeeds without an MCP session (GET may get 405);
//! 3. the tool result reaches the model, so the CLI accepted the JSON responses.
//!
//! It prints the protocol version the CLI negotiated and every HTTP exchange.
//!
//! The tests run real turns on the user's subscriptions, so they are ignored by default. Rerun
//! them after upgrading a CLI or rmcp:
//!
//! ```text
//! cargo test -p lobotomyd --test mcp_contract -- --ignored --nocapture --test-threads 1
//! ```
//!
//! `CODEX_BIN` and `CLAUDE_BIN` override the binaries.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::Body,
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
};
use lobotomy_core::id::new_id;
use lobotomyd::mcp;
use rmcp::{
    RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerConfig},
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

const TURN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize)]
struct ProbeCall {
    token: Option<String>,
    text: String,
    protocol_version: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct ProbeArgs {
    /// Any short text.
    text: String,
}

#[derive(Clone)]
struct Probe {
    ack: String,
    calls: Arc<Mutex<Vec<ProbeCall>>>,
    #[expect(dead_code, reason = "the tool_handler macro builds its own router")]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Probe {
    #[tool(description = "Contract-test probe. Returns an acknowledgement code.")]
    fn probe(
        &self,
        Parameters(ProbeArgs { text }): Parameters<ProbeArgs>,
        context: RequestContext<RoleServer>,
    ) -> String {
        self.calls.lock().unwrap().push(ProbeCall {
            token: mcp::turn_token(&context).map(str::to_owned),
            text,
            protocol_version: context.peer.peer_info().map(|info| info.protocol_version.to_string()),
        });
        self.ack.clone()
    }
}

#[tool_handler]
impl ServerHandler for Probe {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}

/// One HTTP request to the service and its response.
#[derive(Debug, Clone, Serialize)]
struct Exchange {
    http_method: String,
    rpc_method: Option<String>,
    protocol_header: Option<String>,
    /// `params.protocolVersion` of an `initialize` request, or `_meta` of a stateless request.
    requested_version: Option<String>,
    /// `result.protocolVersion` of an `initialize` response.
    answered_version: Option<String>,
    status: u16,
    content_type: Option<String>,
}

type Log = Arc<Mutex<Vec<Exchange>>>;

async fn record(State(log): State<Log>, request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap_or_default();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let protocol_header =
        parts.headers.get("mcp-protocol-version").and_then(|value| value.to_str().ok()).map(str::to_owned);
    let http_method = parts.method.to_string();
    let response = next.run(Request::from_parts(parts, Body::from(bytes))).await;

    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap_or_default();
    let reply: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let text = |value: &Value| value.as_str().map(str::to_owned);
    log.lock().unwrap().push(Exchange {
        http_method,
        rpc_method: text(&json["method"]),
        protocol_header,
        requested_version: text(&json["params"]["protocolVersion"])
            .or_else(|| text(&json["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"])),
        answered_version: text(&reply["result"]["protocolVersion"]),
        status: parts.status.as_u16(),
        content_type: parts.headers.get("content-type").and_then(|value| value.to_str().ok()).map(str::to_owned),
    });
    Response::from_parts(parts, Body::from(bytes))
}

struct Service {
    url: String,
    token: String,
    ack: String,
    calls: Arc<Mutex<Vec<ProbeCall>>>,
    log: Log,
    shutdown: CancellationToken,
}

async fn serve() -> Service {
    let token = new_id("tok");
    let ack = new_id("ack");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let shutdown = CancellationToken::new();

    let probe = Probe { ack: ack.clone(), calls: calls.clone(), tool_router: Probe::tool_router() };
    let app =
        mcp::router(move || probe.clone(), shutdown.clone()).layer(middleware::from_fn_with_state(log.clone(), record));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp/{token}", listener.local_addr().unwrap());
    let stop = shutdown.clone();
    tokio::spawn(async move {
        axum::serve(listener, app).with_graceful_shutdown(stop.cancelled_owned()).await.unwrap();
    });
    Service { url, token, ack, calls, log, shutdown }
}

const PROMPT: &str = "Call the tool `probe` from the MCP server `lobotomy` exactly once, with text \"contract\". \
                      Then reply with exactly the code the tool returned, and nothing else.";

struct Run {
    success: bool,
    stdout: String,
    stderr: String,
}

async fn run(program: &Path, args: &[String], cwd: &Path) -> Run {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap_or_else(|e| panic!("starting {}: {e}", program.display()));
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(PROMPT.as_bytes()).await.unwrap();
    drop(stdin);
    let output =
        tokio::time::timeout(TURN_TIMEOUT, child.wait_with_output()).await.expect("the turn timed out").unwrap();
    Run {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn events(stdout: &str) -> Vec<Value> {
    stdout.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lobotomy-mcp-contract-{name}-{}", new_id("run")));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn version(program: &Path) -> String {
    std::process::Command::new(program)
        .arg("--version")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|e| format!("unknown ({e})"))
}

/// Prints the findings and checks the contract.
fn check(cli: &str, cli_version: &str, service: &Service, run: &Run, final_text: &str) {
    let calls = service.calls.lock().unwrap().clone();
    let log = service.log.lock().unwrap().clone();
    let report = serde_json::json!({
        "cli": cli,
        "cli_version": cli_version,
        "rmcp": "3.5",
        "exit_success": run.success,
        "final_text": final_text,
        "probe_calls": calls,
        "exchanges": log,
    });
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
    if !run.success {
        eprintln!("--- stderr ---\n{}", run.stderr);
    }

    assert!(run.success, "{cli} exited with failure");
    assert!(!calls.is_empty(), "{cli} did not call the probe tool");
    for call in &calls {
        assert_eq!(call.token.as_deref(), Some(service.token.as_str()), "token not read from the URL");
    }
    for exchange in &log {
        let allowed = exchange.status < 400 || (exchange.http_method != "POST" && exchange.status == 405);
        assert!(allowed, "{cli}: unexpected HTTP status in {exchange:?}");
    }
    assert!(final_text.contains(&service.ack), "the tool result did not reach the model");
}

#[tokio::test]
#[ignore = "runs a real Codex turn"]
async fn codex() {
    let service = serve().await;
    let codex = lobotomy_harness::codex::locate();
    let cwd = scratch_dir("codex");
    // The arguments the runtime builds (harness-adapter.md §1.2), plus a flag that keeps the
    // probe out of the user's history.
    let mut args = lobotomy_harness::codex::TurnArgs {
        resume: None,
        model: None,
        reasoning_effort: Some("low".into()),
        permission: lobotomy_harness::Permission::Full,
        developer_instructions: String::new(),
        mcp_url: service.url.clone(),
    }
    .to_args();
    args.insert(1, "--ephemeral".into());

    let result = run(&codex, &args, &cwd).await;
    let final_text: String = events(&result.stdout)
        .iter()
        .filter(|event| event["type"] == "item.completed" && event["item"]["type"] == "agent_message")
        .filter_map(|event| event["item"]["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    service.shutdown.cancel();
    let _ = std::fs::remove_dir_all(&cwd);
    check("codex", &version(&codex), &service, &result, &final_text);
}

#[tokio::test]
#[ignore = "runs a real Claude Code turn"]
async fn claude() {
    let service = serve().await;
    let claude = std::env::var_os("CLAUDE_BIN").map_or_else(|| PathBuf::from("claude"), PathBuf::from);
    let cwd = scratch_dir("claude");
    let config = cwd.join("mcp.json");
    let servers = serde_json::json!({ "mcpServers": { "lobotomy": { "type": "http", "url": service.url } } });
    std::fs::write(&config, servers.to_string()).unwrap();
    let args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--dangerously-skip-permissions",
        "--model",
        "haiku",
        "--strict-mcp-config",
        "--mcp-config",
        &config.to_string_lossy(),
        "--no-session-persistence",
    ]
    .map(str::to_owned)
    .to_vec();

    let result = run(&claude, &args, &cwd).await;
    let events = events(&result.stdout);
    let mcp_status: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "system" && event["subtype"] == "init")
        .map(|event| &event["mcp_servers"])
        .collect();
    eprintln!("claude system/init mcp_servers: {mcp_status:?}");
    let final_text = events
        .iter()
        .rev()
        .find(|event| event["type"] == "result")
        .and_then(|event| event["result"].as_str())
        .unwrap_or_default()
        .to_owned();
    service.shutdown.cancel();
    let _ = std::fs::remove_dir_all(&cwd);
    check("claude", &version(&claude), &service, &result, &final_text);
}
