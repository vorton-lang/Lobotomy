//! Angela's tools (data-model.md §10.6; roadmap.md M4 step 1): she reads every project. A read
//! says what it covers and up to which event; a project it could not read is named with the
//! reason, never left out, so "没有" and "不可用" stay apart (data-model.md §10.6 "读取带边界").
//!
//! Her URL is `/mcp/angela/{token}`: the token finds her turn in her own database.

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, Request},
    response::IntoResponse,
    routing::any,
};
use lobotomy_core::host::ProjectState;
use lobotomy_core::quota::Domain;
use lobotomy_core::turn::turn_by_token;
use lobotomy_core::view::manager::{events_read, project_read, task_read};
use lobotomy_core::view::{last_event, overview};
use lobotomy_harness::{Harness, MCP_SERVER};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig},
    schemars::{self, JsonSchema},
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{TurnToken, turn_token};
use crate::gui::project_attention;
use crate::instance::Instance;
use crate::project::Project;
use crate::projects::Projects;
use crate::runner::db;

/// What her tools read: her instance, for her turn, and the projects.
#[derive(Clone)]
pub struct Reach {
    pub angela: Arc<Instance>,
    pub projects: Arc<Projects>,
}

/// The route of her tools.
pub fn router(reach: Reach, shutdown: CancellationToken) -> Router {
    let service = super::service(AngelaTools::new, shutdown);
    Router::new().route(
        "/mcp/angela/{token}",
        any(move |Path(token): Path<String>, mut request: Request| {
            let (service, reach) = (service.clone(), reach.clone());
            async move {
                request.extensions_mut().insert(TurnToken(token));
                request.extensions_mut().insert(reach);
                service.handle(request).await.into_response()
            }
        }),
    )
}

#[derive(Clone)]
pub struct AngelaTools {
    #[expect(dead_code, reason = "the tool_handler macro builds its own router")]
    tool_router: ToolRouter<Self>,
}

impl AngelaTools {
    pub fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }
}

impl Default for AngelaTools {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectArgs {
    /// 项目 ID，来自 list_projects
    pub project_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskArgs {
    /// 项目 ID，来自 list_projects
    pub project_id: String,
    /// 任务 ID，来自 read_project
    pub task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EventsArgs {
    /// 项目 ID，来自 list_projects
    pub project_id: String,
    /// 只读这个序号之后的事件；不填时从头读
    pub after: Option<i64>,
    /// 最多读多少条，默认 100，最多 500
    pub limit: Option<usize>,
}

const EVENTS_DEFAULT: usize = 100;
const EVENTS_MAX: usize = 500;

#[tool_router]
impl AngelaTools {
    #[tool(description = "列出所有登记的项目：每个项目的状态、截至的事件序号、进行中的任务数和等用户决定的条数；\
                       读不到的项目写明原因。还列出受阻的额度域。")]
    async fn list_projects(&self, context: RequestContext<RoleServer>) -> Result<CallToolResult, ErrorData> {
        answer(&context, |reach| async move { list_projects(&reach).await }).await
    }

    #[tool(description = "读一个项目的现状：各角色在做什么、任务列表（进行中的全部，已关闭的最近 10 个）、\
                       等用户决定的事，以及截至的事件序号。")]
    async fn read_project(
        &self,
        Parameters(args): Parameters<ProjectArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(&context, |reach| async move {
            let project = match find(&reach, &args.project_id)? {
                Ok(project) => project,
                Err(unavailable) => return Ok(unavailable),
            };
            let attention_of = project.clone();
            let overview = db(&project, |db| db.read(|c| overview(c, MCP_SERVER))).await?;
            let attention = project_attention(&attention_of, &overview);
            Ok(json!({ "project": args.project_id, "read": project_read(&overview, &attention) }))
        })
        .await
    }

    #[tool(description = "读一个任务：用户原话、当前完成条件、每一轮执行的候选成果与验证（标明是否已被取代、\
                       验证是否已过期）、执行者的汇报，以及截至的事件序号。")]
    async fn read_task(
        &self,
        Parameters(args): Parameters<TaskArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(&context, |reach| async move {
            let project = match find(&reach, &args.project_id)? {
                Ok(project) => project,
                Err(unavailable) => return Ok(unavailable),
            };
            let task_id = args.task_id.clone();
            let read = db(&project, move |db| {
                db.read(|c| match task_read(c, &task_id) {
                    Err(e) if e.code() == Some("not_found") => Ok(json!({ "as_of": last_event(c)?, "task": null })),
                    read => Ok(serde_json::to_value(read?)?),
                })
            })
            .await?;
            Ok(json!({ "project": args.project_id, "task_id": args.task_id, "read": read }))
        })
        .await
    }

    #[tool(description = "读一个项目的事件：每条事件写明种类、对象和发起者（user、runtime 或 role:<名字>）。\
                       用 after 接着上次读到的序号往后读。")]
    async fn read_events(
        &self,
        Parameters(args): Parameters<EventsArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(&context, |reach| async move {
            let project = match find(&reach, &args.project_id)? {
                Ok(project) => project,
                Err(unavailable) => return Ok(unavailable),
            };
            let (after, limit) = (args.after.unwrap_or(0), args.limit.unwrap_or(EVENTS_DEFAULT).min(EVENTS_MAX));
            let read = db(&project, move |db| db.read(|c| events_read(c, after, limit))).await?;
            Ok(json!({ "project": args.project_id, "read": read }))
        })
        .await
    }
}

#[tool_handler]
impl ServerHandler for AngelaTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Lobotomy：读取所有项目的现状。每次读取写明截至哪个事件，读不到的项目写明原因。")
    }
}

/// Runs a read for Angela's turn and returns it as JSON text. A call whose token is not one of her
/// turns gets nothing.
async fn answer<F, Fut>(context: &RequestContext<RoleServer>, read: F) -> Result<CallToolResult, ErrorData>
where
    F: FnOnce(Reach) -> Fut,
    Fut: Future<Output = anyhow::Result<Value>>,
{
    let reach = context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Reach>().cloned())
        .ok_or_else(|| ErrorData::internal_error("the request did not come through Angela's route", None))?;
    let token = turn_token(context).map(str::to_owned);
    let known = match token {
        Some(token) => db(&reach.angela, move |db| db.read(|c| turn_by_token(c, &token))).await.ok().flatten(),
        None => None,
    };
    if known.is_none() {
        return Ok(CallToolResult::error(vec![ContentBlock::text("读取被拒绝：这个 URL 不属于 Angela 的 turn")]));
    }
    match read(reach).await {
        Ok(value) => Ok(CallToolResult::success(vec![ContentBlock::text(value.to_string())])),
        Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!("读取失败：{e:#}"))])),
    }
}

/// An open project, or what to say instead: no such project, or why it is not open.
fn find(reach: &Reach, id: &str) -> anyhow::Result<Result<Arc<Project>, Value>> {
    if let Some(project) = reach.projects.get(id) {
        return Ok(Ok(project));
    }
    let entry = reach.angela.host.db.project(id)?;
    Ok(Err(json!({ "project": id, "unavailable": unavailable(&reach.projects, id, entry.map(|e| e.state)) })))
}

/// Why a registered project cannot be read now.
fn unavailable(projects: &Projects, id: &str, state: Option<ProjectState>) -> String {
    match state {
        None => "没有这个项目".into(),
        Some(ProjectState::Archived) => "已归档".into(),
        Some(ProjectState::Onboarding) => "正在接入".into(),
        Some(ProjectState::Running) => match projects.failure(id) {
            Some(reason) => format!("打不开：{reason}"),
            None => "没有打开".into(),
        },
    }
}

#[derive(Serialize)]
struct ProjectEntryRead {
    id: String,
    name: String,
    repo_path: String,
    /// Why it cannot be read now; `None` for an open project.
    unavailable: Option<String>,
    /// The last event of the project the counts include.
    as_of: Option<i64>,
    open_tasks: Option<usize>,
    waiting: Option<usize>,
}

async fn list_projects(reach: &Reach) -> anyhow::Result<Value> {
    let mut entries = Vec::new();
    for entry in reach.angela.host.db.projects()? {
        let mut read = ProjectEntryRead {
            id: entry.id.clone(),
            name: entry.name.clone(),
            repo_path: entry.repo_path.clone(),
            unavailable: None,
            as_of: None,
            open_tasks: None,
            waiting: None,
        };
        match reach.projects.get(&entry.id) {
            Some(project) => {
                let attention_of = project.clone();
                let overview = db(&project, |db| db.read(|c| overview(c, MCP_SERVER))).await?;
                read.as_of = Some(overview.seq);
                read.open_tasks = Some(overview.tasks.iter().filter(|t| !t.task.phase.is_closed()).count());
                read.waiting = Some(project_attention(&attention_of, &overview).len());
            }
            None => read.unavailable = Some(unavailable(&reach.projects, &entry.id, Some(entry.state))),
        }
        entries.push(read);
    }
    let blocked: Vec<Domain> = Harness::ALL
        .iter()
        .map(|h| reach.angela.host.db.domain(h.as_str()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(Domain::is_blocked)
        .collect();
    Ok(json!({ "projects": entries, "quota_blocked": blocked }))
}
