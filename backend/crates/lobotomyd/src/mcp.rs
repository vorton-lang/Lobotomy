//! The MCP service that roles call (harness-adapter.md §2).
//!
//! rmcp types stay inside this module (architecture.md §3). Each turn gets its own URL,
//! `/mcp/{token}`, so the URL identifies the caller (data-model.md §3.1) and the service keeps
//! no MCP sessions. Stateless serving also covers clients that still use the `initialize`
//! handshake of the protocol versions before 2026-07-28: rmcp answers each of their requests on
//! its own.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, Request},
    routing::any,
};
use lobotomy_core::Caller;
use lobotomy_core::report::{OrgReport, ReportEffect, ReportStatus};
use lobotomy_core::task::Trial;
use lobotomy_core::turn::turn_by_token;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig},
    schemars::{self, JsonSchema},
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
    },
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::project::Project;

/// The per-turn token from the request URL. The router puts it into the HTTP request extensions
/// before rmcp parses the request.
#[derive(Debug, Clone)]
pub struct TurnToken(pub String);

/// The HTTP routes of the MCP service. rmcp calls `make_handler` once per request.
pub fn router<S>(make_handler: impl Fn() -> S + Send + Sync + 'static, shutdown: CancellationToken) -> Router
where
    S: ServerHandler + Send + 'static,
{
    let service = StreamableHttpService::new(
        move || Ok(make_handler()),
        Arc::new(NeverSessionManager::default()),
        config(shutdown),
    );
    Router::new().route(
        "/mcp/{token}",
        any(move |Path(token): Path<String>, mut request: Request| {
            let service = service.clone();
            async move {
                request.extensions_mut().insert(TurnToken(token));
                service.handle(request).await
            }
        }),
    )
}

fn config(shutdown: CancellationToken) -> StreamableHttpServerConfig {
    // `allowed_hosts` keeps the default: loopback names only, against DNS rebinding.
    StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_sse_keep_alive(None)
        .with_cancellation_token(shutdown)
}

/// The token of the turn that sent this request. `None` when the request did not come through
/// [`router`].
pub fn turn_token(context: &RequestContext<RoleServer>) -> Option<&str> {
    let parts = context.extensions.get::<http::request::Parts>()?;
    parts.extensions.get::<TurnToken>().map(|token| token.0.as_str())
}

/// The MCP routes with the tools roles call.
pub fn org_router(project: Arc<Project>, shutdown: CancellationToken) -> Router {
    router(move || OrgTools::new(project.clone()), shutdown)
}

/// The tools of the executor (roles-and-tasks.md §4). Every call becomes a command of the turn
/// that owns the token; the reply names the command record, so the transcript item can refer to
/// it instead of repeating it (data-model.md §7.2).
#[derive(Clone)]
pub struct OrgTools {
    project: Arc<Project>,
    #[expect(dead_code, reason = "the tool_handler macro builds its own router")]
    tool_router: ToolRouter<Self>,
}

impl OrgTools {
    pub fn new(project: Arc<Project>) -> Self {
        Self { project, tool_router: Self::tool_router() }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatusArg {
    /// 阶段性进展
    Progress,
    /// 需要用户决定的问题，写在 blocked_on 中
    Blocked,
    /// 任务完成，提交候选成果
    Done,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TrialArg {
    /// 适用于当前平台 shell、从候选成果根目录运行的具体命令
    pub command: String,
    /// 用户运行此命令可以体验或验证什么
    pub purpose: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OrgReportArgs {
    /// 一句话标题
    pub title: String,
    /// 详细内容。done 时简述做了什么、验证结果、成果在哪里、怎样体验、重点验证及已知限制；按需提供链接、路径或命令和使用前提
    pub body: String,
    pub status: ReportStatusArg,
    /// status 为 blocked 时，需要用户决定的问题
    pub blocked_on: Option<String>,
    /// 仅 done 可附带的体验建议。只保存元数据，用户明确点击后才会运行，不会自动执行，也不是自动验证检查
    pub trial: Option<TrialArg>,
}

#[tool_router]
impl OrgTools {
    #[tool(
        description = "向 Lobotomy 组织汇报：阶段性进展（progress）、需要用户决定的问题（blocked）或任务完成（done）。"
    )]
    async fn org_report(
        &self,
        Parameters(args): Parameters<OrgReportArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let token = turn_token(&context).map(str::to_owned);
        let project = self.project.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let token =
                token.ok_or_else(|| lobotomy_core::Error::rejected("no_token", "the URL carries no turn token"))?;
            let turn = project
                .db
                .read(|c| turn_by_token(c, &token))?
                .ok_or_else(|| lobotomy_core::Error::rejected("unknown_token", "no turn has this token"))?;
            let caller = Caller::Role { role: turn.role, turn_id: turn.id };
            let status = match args.status {
                ReportStatusArg::Progress => ReportStatus::Progress,
                ReportStatusArg::Blocked => ReportStatus::Blocked,
                ReportStatusArg::Done => ReportStatus::Done,
            };
            let report = OrgReport {
                title: args.title,
                body: args.body,
                status,
                blocked_on: args.blocked_on,
                trial: args.trial.map(|trial| Trial { command: trial.command, purpose: trial.purpose }),
            };
            project.db.execute_recorded(&caller, &report)
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let text = match outcome {
            Ok((ReportEffect::Recorded, id)) => format!("已记录（{id}）。"),
            Ok((ReportEffect::Unchanged, id)) => format!("与之前的汇报相同，没有重复记录（{id}）。"),
            Ok((ReportEffect::Late, id)) => format!("这次汇报没有生效：它所属的执行轮已经结束（{id}）。"),
            Ok((ReportEffect::NoTask, id)) => format!(
                "这次汇报没有生效：这个 turn 不属于任何任务，done 和提问只对执行中的任务有效（{id}）。\
                 你在这个 turn 里做的改动不会进入任何成果；用户会决定把它们建成任务还是丢弃。"
            ),
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(format!("汇报被拒绝：{e}"))])),
        };
        self.project.wake.notify_one();
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for OrgTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Lobotomy 组织运行时。用 org_report 汇报工作状态。")
    }
}

/// The command record named in a Lobotomy tool's reply, such as `cmd_01J…`.
pub fn command_id_in(text: &str) -> Option<&str> {
    let start = text.find("cmd_")?;
    let id = &text[start..];
    let end = id.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(id.len());
    (end == 4 + 26).then(|| &id[..end])
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http::{StatusCode, header};
    use rmcp::{
        handler::server::{router::tool::ToolRouter, wrapper::Parameters},
        model::{ServerCapabilities, ServerConfig},
        schemars, tool, tool_handler, tool_router,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn org_report_trial_is_optional_and_described_in_the_schema() {
        let legacy = json!({ "title": "完成", "body": "已验证", "status": "done" });
        let args: OrgReportArgs = serde_json::from_value(legacy.clone()).unwrap();
        assert!(args.trial.is_none(), "older workers do not need to send trial");

        let mut with_trial = legacy;
        with_trial["trial"] = json!({ "command": "npm run dev", "purpose": "体验页面" });
        let args: OrgReportArgs = serde_json::from_value(with_trial).unwrap();
        let trial = args.trial.unwrap();
        assert_eq!((trial.command.as_str(), trial.purpose.as_str()), ("npm run dev", "体验页面"));

        let schema = serde_json::to_value(schemars::schema_for!(OrgReportArgs)).unwrap();
        assert!(schema["properties"]["trial"].is_object());
        assert!(!schema["required"].as_array().unwrap().contains(&json!("trial")));
        let required = schema["$defs"]["TrialArg"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("command")) && required.contains(&json!("purpose")));
    }

    #[derive(serde::Deserialize, schemars::JsonSchema)]
    struct EchoArgs {
        text: String,
    }

    #[derive(Clone)]
    struct Echo {
        #[expect(dead_code, reason = "the tool_handler macro builds its own router")]
        tool_router: ToolRouter<Self>,
    }

    #[tool_router]
    impl Echo {
        #[tool(description = "Return the caller's token and the text")]
        fn echo(
            &self,
            Parameters(EchoArgs { text }): Parameters<EchoArgs>,
            context: RequestContext<RoleServer>,
        ) -> String {
            format!("{}:{text}", turn_token(&context).unwrap_or("none"))
        }
    }

    #[tool_handler]
    impl ServerHandler for Echo {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
        }
    }

    fn echo_router() -> Router {
        router(|| Echo { tool_router: Echo::tool_router() }, CancellationToken::new())
    }

    struct Reply {
        status: StatusCode,
        content_type: String,
        session_id: Option<String>,
        body: Value,
    }

    async fn send(token: &str, method: &str, headers: &[(&str, &str)], body: Option<Value>) -> Reply {
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("/mcp/{token}"))
            .header(header::HOST, "127.0.0.1:4000")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let body = body.map_or_else(Body::empty, |body| Body::from(body.to_string()));
        let response = echo_router().oneshot(request.body(body).unwrap()).await.unwrap();
        let header =
            |name| response.headers().get(name).map(|value: &http::HeaderValue| value.to_str().unwrap().to_owned());
        let content_type = header(header::CONTENT_TYPE.as_str()).unwrap_or_default();
        let session_id = header("mcp-session-id");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply { status, content_type, session_id, body }
    }

    fn tool_text(reply: &Reply) -> &str {
        reply.body["result"]["content"][0]["text"].as_str().unwrap_or_default()
    }

    #[tokio::test]
    async fn legacy_handshake_is_served_without_a_session() {
        let init = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }
        });
        let reply = send("tok-a", "POST", &[], Some(init)).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert!(reply.content_type.starts_with("application/json"), "{}", reply.content_type);
        assert_eq!(reply.body["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(reply.session_id, None);

        let call = json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "echo", "arguments": { "text": "hi" } }
        });
        let reply = send("tok-a", "POST", &[("mcp-protocol-version", "2025-06-18")], Some(call)).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert!(reply.content_type.starts_with("application/json"), "{}", reply.content_type);
        assert_eq!(tool_text(&reply), "tok-a:hi");
    }

    #[tokio::test]
    async fn stateless_protocol_call_reads_the_token() {
        let call = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {
                "name": "echo",
                "arguments": { "text": "hi" },
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        });
        let headers = [("mcp-protocol-version", "2026-07-28"), ("mcp-method", "tools/call"), ("mcp-name", "echo")];
        let reply = send("tok-b", "POST", &headers, Some(call)).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert!(reply.content_type.starts_with("application/json"), "{}", reply.content_type);
        assert_eq!(tool_text(&reply), "tok-b:hi");
    }

    #[tokio::test]
    async fn get_stream_is_not_offered() {
        let reply = send("tok-c", "GET", &[], None).await;
        assert_eq!(reply.status, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn foreign_host_is_rejected() {
        let request = http::Request::builder()
            .method("POST")
            .uri("/mcp/tok-d")
            .header(header::HOST, "attacker.example")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let response = echo_router().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
