//! Angela's tools (data-model.md §10.6; roadmap.md M4). She reads every project: a read says what
//! it covers and up to which event; a project it could not read is named with the reason, never
//! left out, so "没有" and "不可用" stay apart (data-model.md §10.6 "读取带边界"). She acts in a
//! project as the caller `manager`: assigning, forwarding the user's words, accepting, sending
//! back, abandoning and pausing (roadmap.md M4 step 2).
//!
//! Her URL is `/mcp/angela/{token}`: the token finds her turn in her own database.

use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Router,
    extract::{Path, Request},
    response::IntoResponse,
    routing::any,
};
use lobotomy_agent::mcp::{TurnToken, turn_token};
use lobotomy_agent::runner::db;
use lobotomy_core::Caller;
use lobotomy_core::host::ProjectState;
use lobotomy_core::id::new_id;
use lobotomy_core::ledger::{AddLedgerEntry, CitedEntry, LedgerEntry, LedgerKind, ledger};
use lobotomy_core::manager::AskUser;
use lobotomy_core::quota::Domain;
use lobotomy_core::task::{
    Abandon, Accept, Action, Assign, ContextEntry, Quote, Replied, SendBack, SendToRole, SetPaused, local_minute,
    replied,
};
use lobotomy_core::turn::{TurnState, turn_by_token};
use lobotomy_core::view::attachment::{Attachment, attachment};
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
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::gui::project_attention;
use crate::instance::Instance;
use crate::project::Project;
use crate::projects::Projects;
use crate::workers::{Authority, Workers, worker_of};

/// What her tools read: her instance, for her turn, and the projects.
#[derive(Clone)]
pub struct Reach {
    pub angela: Arc<Instance>,
    pub projects: Arc<Projects>,
}

/// The route of her tools.
pub fn router(reach: Reach, shutdown: CancellationToken) -> Router {
    let service = lobotomy_agent::mcp::service(AngelaTools::new, shutdown);
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionArg {
    /// 只读调查：不改文件
    ReadOnly,
    /// 改文档
    EditDocs,
    /// 改代码
    EditCode,
}

impl From<ActionArg> for Action {
    fn from(arg: ActionArg) -> Self {
        match arg {
            ActionArg::ReadOnly => Action::ReadOnly,
            ActionArg::EditDocs => Action::EditDocs,
            ActionArg::EditCode => Action::EditCode,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AssignArgs {
    /// 项目 ID，来自 list_projects
    pub project_id: String,
    /// 执行者的名字，来自 read_project 的 roles
    pub role: String,
    /// 一句话标题
    pub title: String,
    /// 要原样转给执行者的用户消息的 ID，即输入中【来自 用户 · msg_…】里的 ID。运行时按时间顺序插入原文。没有可引用的原话时留空
    #[serde(default)]
    pub quote: Vec<String>,
    /// 用户当时在回复的对象，原样附上：你的回复（item_…，见 recent_replies），或这个项目中的任务、汇报。用户自己指定过回复对象的消息，运行时自动附上，你不用再写
    #[serde(default)]
    pub context: Vec<String>,
    /// 要原样转给执行者的记录条目的 ID（led_…），例如相关的决定和约束。推测不能转给下层
    #[serde(default)]
    pub ledger: Vec<String>,
    /// 你的解读：你理解的目标、推断的边界、不确定的地方、希望何时回报什么。原话以外的内容都是你的推断
    pub interpretation: String,
    /// 执行者可以做的事，可以多选
    pub actions: Vec<ActionArg>,
    /// 停止条件：做到哪里就停下来汇报，不再扩大范围，例如"得到第一个可用结果就汇报""最多测量一次"
    pub stop_when: String,
    /// 完成条件：成果要满足什么，要能验证
    pub done_when: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendToRoleArgs {
    /// 项目 ID，来自 list_projects
    pub project_id: String,
    /// 收信的 role 的名字
    pub role: String,
    /// 消息关于哪个任务；不关于任务时不填
    pub task_id: Option<String>,
    /// 要原样转给它的用户消息的 ID，即输入中【来自 用户 · msg_…】里的 ID。没有时留空
    #[serde(default)]
    pub quote: Vec<String>,
    /// 用户当时在回复的对象，原样附上：你的回复（item_…，见 recent_replies），或这个项目中的任务、汇报。用户自己指定过回复对象的消息，运行时自动附上，你不用再写
    #[serde(default)]
    pub context: Vec<String>,
    /// 要原样转给它的记录条目的 ID（led_…）。推测不能转给下层
    #[serde(default)]
    pub ledger: Vec<String>,
    /// 你的话。转发原话时写你的解读；没有原话时就是你要说的话
    pub interpretation: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AcceptArgs {
    /// 项目 ID，来自 list_projects
    pub project_id: String,
    /// 任务 ID
    pub task_id: String,
    /// 你对照的完成条件版本，来自 read_task 的 criteria_version
    pub criteria_version: i64,
    /// 用户授权这次合并的那条消息的 ID（msg_…）。改动碰到需要用户授权的路径时必填：先问用户，用户同意后填他那条消息
    pub authorization: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReasonArgs {
    /// 项目 ID，来自 list_projects
    pub project_id: String,
    /// 任务 ID
    pub task_id: String,
    /// 理由，会原样记下；退回时也会原样发给执行者
    pub reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LedgerKindArg {
    /// 目标
    Goal,
    /// 阶段：探索、设计、执行、审查
    Phase,
    /// 已定的事，默认不再重开；必须引用用户原话
    Decision,
    /// 约束，写明范围；必须引用用户原话
    Constraint,
    /// 待定的事，以及在等谁；要用户回答时另用 ask_user，待定条目不会出现在用户的"等你决定"中
    OpenQuestion,
    /// 对用户状态的推测，写上把握；不给下层看
    Hypothesis,
}

impl From<LedgerKindArg> for LedgerKind {
    fn from(arg: LedgerKindArg) -> Self {
        match arg {
            LedgerKindArg::Goal => LedgerKind::Goal,
            LedgerKindArg::Phase => LedgerKind::Phase,
            LedgerKindArg::Decision => LedgerKind::Decision,
            LedgerKindArg::Constraint => LedgerKind::Constraint,
            LedgerKindArg::OpenQuestion => LedgerKind::OpenQuestion,
            LedgerKindArg::Hypothesis => LedgerKind::Hypothesis,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LedgerAddArgs {
    pub kind: LedgerKindArg,
    /// 一句话写清这一条
    pub statement: String,
    /// 依据的用户消息的 ID，即输入中【来自 用户 · msg_…】里的 ID。决定和约束必须有；运行时原样存下原文
    #[serde(default)]
    pub evidence: Vec<String>,
    /// 项目 ID：这一条属于某个项目时填写。不填是用户范围，跨所有项目
    pub project_id: Option<String>,
    /// 任务 ID：这一条只在某个任务里有效时填写，同时填 project_id。任务关闭后，约束失效
    pub task_id: Option<String>,
    /// 推测的把握，例如"低""中""高"
    pub confidence: Option<String>,
    /// 被这一条取代的旧条目的 ID；必须在同一范围，而且是最新的那一条
    pub supersedes: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LedgerReadArgs {
    /// 只读这个项目的记录（和用户范围的记录）；不填时读所有项目
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecentArgs {
    /// 最多列出多少条，默认 10，最多 50
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AskArgs {
    /// 只有用户能决定的一个问题，带上你的推荐和默认选择
    pub question: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PauseArgs {
    /// 项目 ID，来自 list_projects
    pub project_id: String,
    /// 任务 ID
    pub task_id: String,
    /// true 暂停，false 恢复。用户暂停的任务只有用户能恢复
    pub paused: bool,
}

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
            let overview = db(&project, |db| db.read(|c| overview(c, MCP_SERVER))).await?;
            let attention = project_attention(&overview);
            Ok(json!({ "project": args.project_id, "read": project_read(&overview, &attention) }))
        })
        .await
    }

    #[tool(description = "读一个任务：用户原话、当前完成条件、每一轮执行怎样结束、执行者的汇报，以及截至的事件序号。")]
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

    #[tool(description = "派一项任务给项目中的 worker。必须写明允许的动作（actions）和停止条件（stop_when）。\
                       要转达的用户原话用 quote 引用消息 ID，由运行时原样插入，不要自己复述成原话。")]
    async fn assign(
        &self,
        Parameters(args): Parameters<AssignArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        act(&context, &args.project_id.clone(), |reach, project, caller| async move {
            // Only a worker takes tasks (tech-plan.md §7 step 1).
            let name = args.role.clone();
            let role = db(&project, move |db| db.read(|c| lobotomy_core::role::load_role(c, &name))).await?;
            if !role.remote {
                anyhow::bail!("{} 不是 worker；read_project 的 roles 里 remote 为 true 的才能接任务", args.role);
            }
            let context = replied_to(&reach.angela, &project, &args.quote, args.context).await?;
            let quotes = quotes(&reach.angela, args.quote).await?;
            let ledger = cite(&reach.angela, &project, args.ledger).await?;
            let cmd = Assign {
                request_id: new_id("req"),
                executor: args.role,
                title: args.title,
                quotes,
                context,
                ledger,
                interpretation: args.interpretation,
                actions: args.actions.into_iter().map(Action::from).collect(),
                stop_when: args.stop_when,
                done_when: args.done_when,
            };
            let created = db(&project, move |db| db.execute(&caller, &cmd)).await?;
            Ok(format!("已派出任务 {}，排进执行者的队列。", created.id))
        })
        .await
    }

    #[tool(description = "给某个任务的 worker 发消息。要转达的用户原话用 quote 引用消息 ID，\
                       由运行时原样插入，你的话附在后面。")]
    async fn send_to_role(
        &self,
        Parameters(args): Parameters<SendToRoleArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        act(&context, &args.project_id.clone(), |reach, project, caller| async move {
            // A worker takes messages about its task only (worker-interface.md §3).
            let Some(task_id) = args.task_id.clone() else {
                anyhow::bail!("worker 只接关于任务的消息：写上 task_id");
            };
            let Some(worker) = worker_of(&project, &task_id).await? else {
                anyhow::bail!("任务 {task_id} 还没交给 worker，等它开始后再发");
            };
            let context = replied_to(&reach.angela, &project, &args.quote, args.context).await?;
            let quotes = quotes(&reach.angela, args.quote).await?;
            let ledger = cite(&reach.angela, &project, args.ledger).await?;
            let cmd = SendToRole {
                request_id: new_id("req"),
                role: worker.worker.clone(),
                task_id: args.task_id,
                quotes,
                context,
                ledger,
                interpretation: args.interpretation,
            };
            let created = db(&project, move |db| db.execute(&caller, &cmd)).await?;
            workers(&reach)?.forward(&project, &worker, &task_id, &created.id).await?;
            Ok(format!("已转给 worker {}（{}）。", worker.worker, created.id))
        })
        .await
    }

    #[tool(description = "验收一个等待验收的任务，任务完成。写明你对照的完成条件版本；它之后有变化时验收被拒绝，\
                       先重新读任务。worker 的任务：按你读到的版本合并它的 PR 再关闭；改动碰到需要用户授权的路径时，\
                       必须在 authorization 填上用户同意的那条消息的 ID。")]
    async fn accept(
        &self,
        Parameters(args): Parameters<AcceptArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        act(&context, &args.project_id.clone(), |reach, project, caller| async move {
            if let Some(worker) = worker_of(&project, &args.task_id).await? {
                let authority = authority(&reach, args.authorization).await?;
                let workers = workers(&reach)?;
                let (task_id, version) = (&args.task_id, args.criteria_version);
                return workers.adopt(&project, caller, &worker, task_id, version, Vec::new(), authority).await;
            }
            let cmd = Accept {
                request_id: new_id("req"),
                task_id: args.task_id,
                criteria_version: args.criteria_version,
                dropping: Vec::new(),
            };
            db(&project, move |db| db.execute(&caller, &cmd)).await?;
            Ok("已验收，任务完成。".to_owned())
        })
        .await
    }

    #[tool(description = "退回一个等待验收的任务，执行者开始新一轮。理由原样发给执行者；worker 在同一个会话里接着改。")]
    async fn send_back(
        &self,
        Parameters(args): Parameters<ReasonArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        act(&context, &args.project_id.clone(), |reach, project, caller| async move {
            if let Some(worker) = worker_of(&project, &args.task_id).await? {
                // Review rework: a message to the same attempt (worker-interface.md §0 rule 4).
                let cmd = SendToRole {
                    request_id: new_id("req"),
                    role: worker.worker.clone(),
                    task_id: Some(args.task_id.clone()),
                    quotes: Vec::new(),
                    context: Vec::new(),
                    ledger: Vec::new(),
                    interpretation: args.reason,
                };
                let created = db(&project, move |db| db.execute(&caller, &cmd)).await?;
                workers(&reach)?.forward(&project, &worker, &args.task_id, &created.id).await?;
                return Ok("已把意见交给 worker，它接着改。".to_owned());
            }
            let cmd = SendBack { request_id: new_id("req"), task_id: args.task_id, reason: args.reason };
            db(&project, move |db| db.execute(&caller, &cmd)).await?;
            Ok("已退回，执行者开始新一轮。".to_owned())
        })
        .await
    }

    #[tool(description = "放弃一个任务：不再推进，成果与记录保留。worker 的任务：worker 停下，PR 关掉。")]
    async fn abandon(
        &self,
        Parameters(args): Parameters<ReasonArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        act(&context, &args.project_id.clone(), |reach, project, caller| async move {
            if let Some(worker) = worker_of(&project, &args.task_id).await? {
                workers(&reach)?.abandon(&project, caller, &worker, &args.task_id, args.reason).await?;
                return Ok("已放弃；worker 停下它的工作，PR 会被关掉。".to_owned());
            }
            let cmd = Abandon { request_id: new_id("req"), task_id: args.task_id, reason: args.reason };
            db(&project, move |db| db.execute(&caller, &cmd)).await?;
            Ok("已放弃。".to_owned())
        })
        .await
    }

    #[tool(description = "向用户提一个只有用户能决定的问题。它列进「等你决定」，直到用户在提问之后发来的消息送到你。\
                       要让相关任务在回答前停下，另外用 set_paused 暂停它们。")]
    async fn ask_user(
        &self,
        Parameters(args): Parameters<AskArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let (reach, turn) = caller_turn(&context).await?;
        let refused = |text: String| Ok(CallToolResult::error(vec![ContentBlock::text(format!("被拒绝：{text}"))]));
        let Some(turn) = turn else {
            return refused("这个 URL 不属于 Angela 的 turn".into());
        };
        let caller = Caller::Role { role: turn.role, turn_id: turn.id };
        let ask = AskUser { question: args.question };
        match db(&reach.angela, move |db| db.execute(&caller, &ask)).await {
            Ok(id) => Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "已列进「等你决定」（{id}）。用户之后发来的消息送到你时，这个问题关闭。"
            ))])),
            Err(e) => refused(why(&e)),
        }
    }

    #[tool(
        description = "在记录中加一条：目标、阶段、决定、约束、待定或推测。只能追加；要改，就加一条新的并用 supersedes \
                       取代旧的。决定和约束必须用 evidence 引用用户原话，没有原话依据的判断记为推测。"
    )]
    async fn ledger_add(
        &self,
        Parameters(args): Parameters<LedgerAddArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let (reach, turn) = caller_turn(&context).await?;
        let refused = |text: String| Ok(CallToolResult::error(vec![ContentBlock::text(format!("被拒绝：{text}"))]));
        let Some(turn) = turn.filter(|t| t.state == TurnState::Running) else {
            return refused("这个 URL 不属于 Angela 正在运行的 turn".into());
        };
        let evidence = match quotes(&reach.angela, args.evidence).await {
            Ok(evidence) => evidence,
            Err(e) => return refused(format!("{e:#}")),
        };
        let entry = AddLedgerEntry {
            kind: args.kind.into(),
            statement: args.statement,
            evidence,
            task_id: args.task_id,
            confidence: args.confidence,
            supersedes: args.supersedes,
        };
        let added = match &args.project_id {
            // A project's or a task's entry is in the project's database (data-model.md §10.6).
            Some(project_id) => {
                let project = match find(&reach, project_id) {
                    Ok(Ok(project)) => project,
                    Ok(Err(unavailable)) => return refused(format!("项目不可用：{}", unavailable["unavailable"])),
                    Err(e) => return refused(format!("{e:#}")),
                };
                let caller = Caller::Manager { turn_id: turn.id };
                db(&project, move |db| db.execute(&caller, &entry)).await
            }
            None if entry.task_id.is_some() => return refused("任务范围的条目要同时写 project_id".into()),
            None => {
                let caller = Caller::Role { role: turn.role, turn_id: turn.id };
                db(&reach.angela, move |db| db.execute(&caller, &entry)).await
            }
        };
        match added {
            Ok(id) => Ok(CallToolResult::success(vec![ContentBlock::text(format!("已记下（{id}）。"))])),
            Err(e) => refused(why(&e)),
        }
    }

    #[tool(
        description = "读记录：用户范围的条目，以及各项目和任务范围的条目。每条写明是否仍然有效：被取代的写明被谁取代，\
                       任务关闭后失效的约束标为 lapsed。读不到的项目写明原因。"
    )]
    async fn read_ledger(
        &self,
        Parameters(args): Parameters<LedgerReadArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(&context, |reach| async move {
            Ok(serde_json::to_value(ledgers(&reach.angela, &reach.projects, args.project_id.as_deref(), true).await?)?)
        })
        .await
    }

    #[tool(
        description = "列出你最近的回复，新的在前：ID、时间和第一行。转达用户的话时，用它找到用户当时在回复的那一条，\
                       把 ID 填进 context。"
    )]
    async fn recent_replies(
        &self,
        Parameters(args): Parameters<RecentArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        answer(&context, |reach| async move {
            let limit = args.limit.unwrap_or(10).min(50) as i64;
            let replies = db(&reach.angela, move |db| {
                db.read(|c| {
                    let mut stmt = c.prepare(
                        "SELECT id, created_at, json_extract(content, '$.text') FROM item WHERE kind = 'agent_message'
                         ORDER BY created_at DESC, id DESC LIMIT ?1",
                    )?;
                    let rows = stmt.query_map([limit], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?))
                    })?;
                    let mut out = Vec::new();
                    for row in rows {
                        let (id, at, text) = row?;
                        let first: String =
                            text.unwrap_or_default().lines().next().unwrap_or_default().chars().take(100).collect();
                        out.push(json!({ "id": id, "at": local_minute(c, at)?, "first_line": first }));
                    }
                    Ok(out)
                })
            })
            .await?;
            Ok(json!({ "replies": replies }))
        })
        .await
    }

    #[tool(description = "暂停或恢复一个任务。用户暂停的任务只有用户能恢复；你暂停的任务你可以恢复。\
                       worker 的任务只能暂停：打断它当前的 turn；让它继续就给它发消息。")]
    async fn set_paused(
        &self,
        Parameters(args): Parameters<PauseArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        act(&context, &args.project_id.clone(), |reach, project, caller| async move {
            if let Some(worker) = worker_of(&project, &args.task_id).await? {
                if !args.paused {
                    anyhow::bail!("worker 的任务不用恢复：给它发一条消息（send_to_role），它就接着做");
                }
                workers(&reach)?.interrupt(&worker, &args.task_id).await?;
                return Ok("已打断 worker 当前的 turn；它停下等你，发消息让它继续。".to_owned());
            }
            let paused = args.paused;
            let cmd = SetPaused { request_id: new_id("req"), task_id: args.task_id, paused };
            db(&project, move |db| db.execute(&caller, &cmd)).await?;
            Ok(if paused { "已暂停。" } else { "已恢复。" }.to_owned())
        })
        .await
    }
}

#[tool_handler]
impl ServerHandler for AngelaTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Lobotomy：读取所有项目的现状，并在项目中派活、转达用户原话、验收、退回、放弃、暂停。\
             每次读取写明截至哪个事件，读不到的项目写明原因。",
        )
    }
}

/// The request's reach and the turn of Angela its token names, if any.
async fn caller_turn(
    context: &RequestContext<RoleServer>,
) -> Result<(Reach, Option<lobotomy_core::turn::Turn>), ErrorData> {
    let reach = context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Reach>().cloned())
        .ok_or_else(|| ErrorData::internal_error("the request did not come through Angela's route", None))?;
    let token = turn_token(context).map(str::to_owned);
    let turn = match token {
        Some(token) => db(&reach.angela, move |db| db.read(|c| turn_by_token(c, &token))).await.ok().flatten(),
        None => None,
    };
    Ok((reach, turn))
}

/// Runs a read for Angela's turn and returns it as JSON text. A call whose token is not one of her
/// turns gets nothing.
async fn answer<F, Fut>(context: &RequestContext<RoleServer>, read: F) -> Result<CallToolResult, ErrorData>
where
    F: FnOnce(Reach) -> Fut,
    Fut: Future<Output = anyhow::Result<Value>>,
{
    let (reach, turn) = caller_turn(context).await?;
    if turn.is_none() {
        return Ok(CallToolResult::error(vec![ContentBlock::text("读取被拒绝：这个 URL 不属于 Angela 的 turn")]));
    }
    match read(reach).await {
        Ok(value) => Ok(CallToolResult::success(vec![ContentBlock::text(value.to_string())])),
        Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(format!("读取失败：{e:#}"))])),
    }
}

/// Runs Angela's command in a project as `manager` (data-model.md §10.6 跨库操作第 2 条). Her
/// turn must still be running in her database; the command then writes the project's database
/// only (第 5 条), and the project's scheduler wakes for what it changed.
async fn act<F, Fut>(
    context: &RequestContext<RoleServer>,
    project_id: &str,
    run: F,
) -> Result<CallToolResult, ErrorData>
where
    F: FnOnce(Reach, Arc<Project>, Caller) -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    let refused = |text: String| Ok(CallToolResult::error(vec![ContentBlock::text(format!("被拒绝：{text}"))]));
    let (reach, turn) = caller_turn(context).await?;
    let Some(turn) = turn else {
        return refused("这个 URL 不属于 Angela 的 turn".into());
    };
    if turn.state != TurnState::Running {
        return refused("这个 turn 已经结束".into());
    }
    let project = match find(&reach, project_id) {
        Ok(Ok(project)) => project,
        Ok(Err(unavailable)) => return refused(format!("项目不可用：{}", unavailable["unavailable"])),
        Err(e) => return refused(format!("{e:#}")),
    };
    let caller = Caller::Manager { turn_id: turn.id };
    match run(reach, project.clone(), caller).await {
        Ok(text) => {
            project.wake.notify_one();
            Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
        }
        Err(e) => refused(why(&e)),
    }
}

/// A refusal in the words Angela acts on.
fn why(e: &anyhow::Error) -> String {
    match e.downcast_ref::<lobotomy_core::Error>().and_then(lobotomy_core::Error::code) {
        Some("paused_by_user") => "用户暂停了这个任务，只有用户能恢复。".into(),
        Some("undelivered_messages") => "执行者还没收到用户发给这个任务的消息，只有用户能放弃它们。\
                                          可以退回任务，让执行者收到这些消息；或者问用户。"
            .into(),
        Some("stale_criteria") => {
            format!("你看到的已经不是当前状态（{e:#}）。先重新读取这个任务。")
        }
        _ => format!("{e:#}"),
    }
}

/// Her ledger across the scopes (manager-actions.md §3; data-model.md §10.6 "记录按作用域存放"):
/// the user's scope from her database, and each project's, with its tasks', from the project's. A
/// project that cannot be read is named with the reason.
#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct Ledgers {
    pub user: Vec<LedgerEntry>,
    pub projects: Vec<ProjectLedger>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
pub struct ProjectLedger {
    pub id: String,
    pub name: String,
    pub entries: Vec<LedgerEntry>,
    /// Why it could not be read; its entries are then empty.
    pub unavailable: Option<String>,
}

/// Her ledger, of one project (`only`) or of all; `guesses` keeps her hypotheses, which the user's
/// view leaves out (manager-actions.md §3.3).
pub async fn ledgers(
    angela: &Instance,
    projects: &Projects,
    only: Option<&str>,
    guesses: bool,
) -> anyhow::Result<Ledgers> {
    let keep = move |entries: Vec<LedgerEntry>| -> Vec<LedgerEntry> {
        entries.into_iter().filter(|e| guesses || e.kind != LedgerKind::Hypothesis).collect()
    };
    let user = keep(db(angela, |db| db.read(ledger)).await?);
    let mut out = Vec::new();
    for entry in angela.host.db.projects()? {
        if only.is_some_and(|id| id != entry.id) || (only.is_none() && entry.state == ProjectState::Archived) {
            continue;
        }
        let (entries, unavailable) = match projects.get(&entry.id) {
            Some(project) => (keep(db(&project, |db| db.read(ledger)).await?), None),
            None => (Vec::new(), Some(unavailable(projects, &entry.id, Some(entry.state)))),
        };
        out.push(ProjectLedger { id: entry.id, name: entry.name, entries, unavailable });
    }
    Ok(Ledgers { user, projects: out })
}

/// What the user was replying to, for the user's words she forwards (manager-actions.md §1.1):
/// first what the user chose for each quoted message, which she cannot leave out, then what she
/// adds. Her replies and her conversation's messages come from her database; tasks and reports
/// from the project's.
async fn replied_to(
    angela: &Instance,
    project: &Project,
    quoted: &[String],
    extra: Vec<String>,
) -> anyhow::Result<Vec<ContextEntry>> {
    let quoted = quoted.to_vec();
    let mut out: Vec<ContextEntry> = db(angela, move |db| {
        db.read(|c| {
            let mut chosen = Vec::new();
            for id in &quoted {
                let target: Option<String> =
                    c.query_row("SELECT reply_to FROM message WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
                if let Some(target) = target.and_then(|t| replied(c, &t).transpose()) {
                    chosen.push(reply_context(c, target?, true)?);
                }
            }
            Ok(chosen)
        })
    })
    .await?;
    for id in extra {
        if out.iter().any(|c| c.source_id == id) {
            continue;
        }
        let own = id.clone();
        let theirs = id.clone();
        let entry = match db(angela, move |db| db.read(|c| replied(c, &own))).await? {
            Some(target) => db(angela, move |db| db.read(|c| reply_context(c, target, false))).await?,
            None => match db(project, move |db| db.read(|c| record_context(c, &theirs))).await? {
                Some(entry) => entry,
                None => anyhow::bail!("没有 {id}：上下文可以是你的回复（recent_replies），或这个项目中的任务、汇报"),
            },
        };
        out.push(entry);
    }
    Ok(out)
}

fn reply_context(c: &rusqlite::Connection, target: Replied, chosen: bool) -> lobotomy_core::Result<ContextEntry> {
    let header = format!("{} {}（{}）", target.from, local_minute(c, target.at)?, target.id);
    Ok(ContextEntry { source_id: target.id, header, body: target.text, chosen })
}

/// A record of the project as context: its line, and a report's body.
fn record_context(c: &rusqlite::Connection, id: &str) -> lobotomy_core::Result<Option<ContextEntry>> {
    let Some(record) = attachment(c, id)? else { return Ok(None) };
    let (header, body) = match record {
        Attachment::Task { title, .. } => (format!("任务「{title}」"), String::new()),
        Attachment::Report { role, report_title, .. } => {
            let args: String = c.query_row("SELECT args FROM command_record WHERE id = ?1", [id], |r| r.get(0))?;
            let report: serde_json::Value = serde_json::from_str(&args)?;
            let body = report["body"].as_str().unwrap_or_default().to_owned();
            (format!("{role} 的汇报「{report_title}」"), body)
        }
    };
    Ok(Some(ContextEntry { source_id: id.to_owned(), header: format!("{header}（{id}）"), body, chosen: false }))
}

/// The ledger entries she cites to a role, looked up in her database and the project's: each must
/// still hold, and a hypothesis is never passed on (manager-actions.md §3.4).
async fn cite(angela: &Instance, project: &Project, ids: Vec<String>) -> anyhow::Result<Vec<CitedEntry>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut all = db(angela, |db| db.read(ledger)).await?;
    all.extend(db(project, |db| db.read(ledger)).await?);
    ids.iter()
        .map(|id| {
            let Some(e) = all.iter().find(|e| &e.id == id) else {
                anyhow::bail!("没有记录 {id}：只能引用用户范围和这个项目的记录");
            };
            if e.kind == LedgerKind::Hypothesis {
                anyhow::bail!("{id} 是推测，不传给下层");
            }
            if let Some(newer) = &e.superseded_by {
                anyhow::bail!("{id} 已被 {newer} 取代");
            }
            if e.lapsed {
                anyhow::bail!("{id} 所属的任务已关闭，它已失效");
            }
            Ok(CitedEntry {
                id: e.id.clone(),
                kind: e.kind,
                statement: e.statement.clone(),
                evidence: e.evidence.clone(),
            })
        })
        .collect()
}

/// The user's messages Angela quotes, from her database, in the order they were sent. Each must
/// exist and be the user's own (manager-actions.md §2.1 "引用用户原话").
async fn quotes(angela: &Instance, ids: Vec<String>) -> anyhow::Result<Vec<Quote>> {
    let found = db(angela, move |db| {
        db.read(|c| {
            let mut found = Vec::new();
            for id in &ids {
                let row: Option<(String, i64, String, i64)> = c
                    .query_row("SELECT source, created_at, body, seq FROM message WHERE id = ?1", [id], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })
                    .optional()?;
                found.push((id.clone(), row));
            }
            Ok(found)
        })
    })
    .await?;
    let mut quotes = Vec::new();
    for (id, row) in found {
        match row {
            None => anyhow::bail!("没有消息 {id}"),
            Some((source, ..)) if source != Caller::User.scope() => anyhow::bail!("{id} 不是用户的消息"),
            Some((_, said_at, body, seq)) => quotes.push((seq, Quote { source_id: id, said_at, body })),
        }
    }
    quotes.sort_by_key(|(seq, _)| *seq);
    quotes.dedup_by(|a, b| a.0 == b.0);
    Ok(quotes.into_iter().map(|(_, quote)| quote).collect())
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
                let overview = db(&project, |db| db.read(|c| overview(c, MCP_SERVER))).await?;
                read.as_of = Some(overview.seq);
                read.open_tasks = Some(overview.tasks.iter().filter(|t| !t.task.phase.is_closed()).count());
                read.waiting = Some(project_attention(&overview).len());
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

/// The backend's workers.
fn workers(reach: &Reach) -> anyhow::Result<Arc<Workers>> {
    reach.angela.host.workers.get().cloned().context("the backend runs no workers")
}

/// The authority she names for adopting a change on protected paths: the user's message, checked
/// to be the user's in her database (tech-plan.md §4.6).
async fn authority(reach: &Reach, message: Option<String>) -> anyhow::Result<Authority> {
    let Some(message) = message else { return Ok(Authority::None) };
    let id = message.clone();
    let source: Option<String> = db(&reach.angela, move |db| {
        db.read(|c| Ok(c.query_row("SELECT source FROM message WHERE id = ?1", [&id], |r| r.get(0)).optional()?))
    })
    .await?;
    if source.as_deref() != Some("user") {
        anyhow::bail!("{message} 不是用户发来的消息，不能作为授权");
    }
    Ok(Authority::Message(message))
}
