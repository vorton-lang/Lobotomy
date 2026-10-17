//! The MCP service that roles call (harness-adapter.md §2), and the executor's tools.
//!
//! rmcp types stay inside this module and the tools built on it (architecture.md §3). Each turn
//! gets its own URL, `/mcp/{instance}/{token}`, so the URL identifies the caller (data-model.md
//! §3.1, §10.3) and the service keeps no MCP sessions. Stateless serving also covers clients that
//! still use the `initialize` handshake of the protocol versions before 2026-07-28: rmcp answers
//! each of their requests on its own.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, Request},
    http::StatusCode,
    response::IntoResponse,
    routing::any,
};
use lobotomy_core::report::{FlagMismatch, OrgReport, ReportEffect, ReportStatus};
use lobotomy_core::turn::turn_by_token;
use lobotomy_core::{Caller, Db};
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
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::instance::Instance;

/// The per-turn token from the request URL. The router puts it into the HTTP request extensions
/// before rmcp parses the request.
#[derive(Debug, Clone)]
pub struct TurnToken(pub String);

/// The HTTP routes of the MCP service. `locate` finds what the URL's instance segment names, which
/// goes into the request extensions with the token; an unknown instance gets 404. rmcp calls
/// `make_handler` once per request.
pub fn router<S, P>(
    make_handler: impl Fn() -> S + Send + Sync + 'static,
    locate: impl Fn(&str) -> Option<P> + Clone + Send + Sync + 'static,
    shutdown: CancellationToken,
) -> Router
where
    S: ServerHandler + Send + 'static,
    P: Clone + Send + Sync + 'static,
{
    let service = service(make_handler, shutdown);
    Router::new().route(
        "/mcp/{instance}/{token}",
        any(move |Path((instance, token)): Path<(String, String)>, mut request: Request| {
            let (service, found) = (service.clone(), locate(&instance));
            async move {
                let Some(found) = found else {
                    return StatusCode::NOT_FOUND.into_response();
                };
                request.extensions_mut().insert(TurnToken(token));
                request.extensions_mut().insert(found);
                service.handle(request).await.into_response()
            }
        }),
    )
}

/// Serves the tools `make_handler` makes, keeping no MCP sessions.
pub fn service<S: ServerHandler + Send + 'static>(
    make_handler: impl Fn() -> S + Send + Sync + 'static,
    shutdown: CancellationToken,
) -> StreamableHttpService<S, NeverSessionManager> {
    StreamableHttpService::new(move || Ok(make_handler()), Arc::new(NeverSessionManager::default()), config(shutdown))
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

/// What an executor's tool call acts on: the database of the instance the URL names, and its
/// scheduler to wake. `locate` in [`router`] returns it for [`OrgTools`].
#[derive(Clone)]
pub struct ToolTarget {
    pub db: Arc<Db>,
    pub wake: Arc<Notify>,
}

impl ToolTarget {
    pub fn of<H>(instance: &Instance<H>) -> Self {
        Self { db: instance.db.clone(), wake: instance.wake.clone() }
    }
}

/// The target the request's URL names. `None` when the request did not come through [`router`]
/// with a [`ToolTarget`].
fn request_target(context: &RequestContext<RoleServer>) -> Option<ToolTarget> {
    let parts = context.extensions.get::<http::request::Parts>()?;
    parts.extensions.get::<ToolTarget>().cloned()
}

/// The tools of the executor (roles-and-tasks.md §4). Every call becomes a command of the turn
/// that owns the token, in the instance the URL names; the reply names the command record, so the
/// transcript item can refer to it instead of repeating it (data-model.md §7.2).
#[derive(Clone)]
pub struct OrgTools {
    #[expect(dead_code, reason = "the tool_handler macro builds its own router")]
    tool_router: ToolRouter<Self>,
}

impl OrgTools {
    pub fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }
}

impl Default for OrgTools {
    fn default() -> Self {
        Self::new()
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
pub struct OrgReportArgs {
    /// 一句话标题
    pub title: String,
    /// 详细内容。done 时简述做了什么、验证结果、成果在哪里、怎样体验、重点验证及已知限制；按需提供链接、路径或命令和使用前提
    pub body: String,
    pub status: ReportStatusArg,
    /// status 为 blocked 时，需要用户决定的问题
    pub blocked_on: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FlagMismatchArgs {
    /// 原话与解读哪里不一致，你按哪个理解在做
    pub body: String,
}

#[tool_router]
impl OrgTools {
    #[tool(
        description = "你认为 Angela 的解读与【用户原话】不一致时调用。运行时把它直接放进用户与 Angela 的对话，不经 Angela 转述。"
    )]
    async fn flag_mismatch(
        &self,
        Parameters(args): Parameters<FlagMismatchArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let token = turn_token(&context).map(str::to_owned);
        let Some(target) = request_target(&context) else {
            return Err(ErrorData::internal_error("the URL names no open instance", None));
        };
        let db = target.db.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let token =
                token.ok_or_else(|| lobotomy_core::Error::rejected("no_token", "the URL carries no turn token"))?;
            let turn = db
                .read(|c| turn_by_token(c, &token))?
                .ok_or_else(|| lobotomy_core::Error::rejected("unknown_token", "no turn has this token"))?;
            let caller = Caller::Role { role: turn.role, turn_id: turn.id };
            db.execute_recorded(&caller, &FlagMismatch { body: args.body })
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        match outcome {
            Ok(((), id)) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "已直接告诉用户（{id}）。在得到答复前，按用户原话理解。"
            ))])),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!("被拒绝：{e}"))])),
        }
    }

    #[tool(
        description = "向 Lobotomy 组织汇报：阶段性进展（progress）、需要用户决定的问题（blocked）或任务完成（done）。"
    )]
    async fn org_report(
        &self,
        Parameters(args): Parameters<OrgReportArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let token = turn_token(&context).map(str::to_owned);
        let Some(target) = request_target(&context) else {
            return Err(ErrorData::internal_error("the URL names no open instance", None));
        };
        let db = target.db.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let token =
                token.ok_or_else(|| lobotomy_core::Error::rejected("no_token", "the URL carries no turn token"))?;
            let turn = db
                .read(|c| turn_by_token(c, &token))?
                .ok_or_else(|| lobotomy_core::Error::rejected("unknown_token", "no turn has this token"))?;
            let caller = Caller::Role { role: turn.role, turn_id: turn.id };
            let status = match args.status {
                ReportStatusArg::Progress => ReportStatus::Progress,
                ReportStatusArg::Blocked => ReportStatus::Blocked,
                ReportStatusArg::Done => ReportStatus::Done,
            };
            let report = OrgReport { title: args.title, body: args.body, status, blocked_on: args.blocked_on };
            db.execute_recorded(&caller, &report)
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

        let text = match outcome {
            Ok((ReportEffect::Recorded, id)) => format!("已记录（{id}）。"),
            Ok((ReportEffect::Unchanged, id)) => format!("与之前的汇报相同，没有重复记录（{id}）。"),
            Ok((ReportEffect::Late, id)) => format!("这次汇报没有生效：它所属的执行轮已经结束（{id}）。"),
            Ok((ReportEffect::NoTask, id)) => {
                format!("这次汇报没有生效：这个 turn 不属于任何任务，done 和提问只对执行中的任务有效（{id}）。")
            }
            Err(e) => return Ok(CallToolResult::error(vec![ContentBlock::text(format!("汇报被拒绝：{e}"))])),
        };
        target.wake.notify_one();
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
