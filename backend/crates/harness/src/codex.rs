//! The Codex CLI adapter: binary location, arguments and `exec --json` events
//! (harness-adapter.md §1.2, §1.5, §1.6).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::event::{Event, Item, ItemKind};
use crate::{MCP_SERVER, Model, Permission};

/// Finds the Codex binary. `CODEX_BIN` wins. The desktop app ships the CLI under
/// `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe`, where the hash changes with updates, so
/// the newest one is taken. Otherwise `codex` from PATH.
pub fn locate() -> PathBuf {
    if let Some(path) = std::env::var_os("CODEX_BIN") {
        return path.into();
    }
    let newest = std::env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join("OpenAI").join("Codex").join("bin"))
        .and_then(|bin| std::fs::read_dir(bin).ok())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let exe = entry.ok()?.path().join("codex.exe");
            let modified = exe.metadata().ok()?.modified().ok()?;
            Some((modified, exe))
        })
        .max();
    newest.map_or_else(|| PathBuf::from("codex"), |(_, exe)| exe)
}

/// What one Codex turn needs. The message goes through stdin.
#[derive(Clone, Debug)]
pub struct TurnArgs {
    /// The session to resume; `None` for the first turn of a native session.
    pub resume: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission: Permission,
    /// The role instructions. Codex keeps them across resume and compaction, and passing them
    /// again does no harm (harness-adapter.md §1.4).
    pub developer_instructions: String,
    /// The MCP URL with this turn's token.
    pub mcp_url: String,
    /// The `SKILL.md` files the turn leaves out ([`user_skills`]).
    pub disabled_skills: Vec<PathBuf>,
}

/// The skills the user installed for their own use: each `SKILL.md` under `$CODEX_HOME/skills`
/// (by default `~/.codex/skills`) and `~/.agents/skills`, outside hidden directories such as
/// `.system`, where Codex keeps the skills it ships. Roles leave the user's skills out and keep
/// those of Codex and its plugins (harness-adapter.md §1.6). A repository's own skills stay.
pub fn user_skills() -> Vec<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let codex_home =
        std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".codex")));
    let mut found = Vec::new();
    for root in
        [codex_home.map(|c| c.join("skills")), home.map(|h| h.join(".agents").join("skills"))].into_iter().flatten()
    {
        collect_skills(&root, 0, &mut found);
    }
    found.sort();
    found
}

/// The skills in `dir`: a directory with a `SKILL.md` is one; others are looked into, two levels
/// at most.
fn collect_skills(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') || !path.is_dir() {
            continue;
        }
        let skill = path.join("SKILL.md");
        if skill.is_file() {
            found.push(skill);
        } else if depth < 2 {
            collect_skills(&path, depth + 1, found);
        }
    }
}

/// Turns the skills off one by one: `skills.config` entries name a `SKILL.md` by path, with
/// forward slashes as tested on Windows.
fn skills_config(skills: &[PathBuf]) -> String {
    let entries: Vec<String> = skills
        .iter()
        .map(|p| format!("{{path={}, enabled=false}}", toml_string(&p.to_string_lossy().replace('\\', "/"))))
        .collect();
    format!("skills.config=[{}]", entries.join(", "))
}

/// TOML basic strings accept JSON string syntax.
fn toml_string(s: &str) -> String {
    serde_json::to_string(s).expect("strings serialize")
}

/// The arguments of a permission mode (harness-adapter.md §1.9). Auto review is what
/// `--approve-for-me` sets; that flag exists only before `exec`'s subcommands, so the settings
/// are passed one by one, the same for a first turn and a resumed one.
fn permission_args(permission: Permission) -> Vec<String> {
    match permission {
        Permission::Full => vec!["--dangerously-bypass-approvals-and-sandbox".into()],
        Permission::AutoReview => {
            ["approval_policy=\"on-request\"", "approvals_reviewer=\"auto_review\"", "sandbox_mode=\"workspace-write\""]
                .into_iter()
                .flat_map(|setting| ["-c".to_owned(), setting.to_owned()])
                .collect()
        }
    }
}

/// Whether the CLI's stderr says the administrator's requirements do not allow the permission
/// mode it was given. Codex 0.159.2 writes, and exits before any output on stdout:
/// "Error: `approval_policy = "never"` cannot be used because requirements do not allow
/// `sandbox_mode = "danger-full-access"`; …" (#13).
pub fn permission_refused(stderr: &str) -> bool {
    stderr.contains("because requirements do not allow")
}

impl TurnArgs {
    pub fn to_args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec!["exec".into()];
        if let Some(id) = &self.resume {
            args.extend(["resume".into(), id.clone()]);
        }
        args.push("--json".into());
        args.extend(permission_args(self.permission));
        args.extend(TRIMMED.map(String::from));
        if let Some(model) = &self.model {
            args.extend(["-m".into(), model.clone()]);
        }
        if let Some(effort) = &self.reasoning_effort {
            args.extend(["-c".into(), format!("model_reasoning_effort={}", toml_string(effort))]);
        }
        if !self.disabled_skills.is_empty() {
            args.extend(["-c".into(), skills_config(&self.disabled_skills)]);
        }
        args.extend([
            "-c".into(),
            format!("developer_instructions={}", toml_string(&self.developer_instructions)),
            "-c".into(),
            format!("mcp_servers.{MCP_SERVER}.url={}", toml_string(&self.mcp_url)),
            // Lobotomy's own tools never wait for an approval. Outside full access, Codex would
            // otherwise refuse org_report: "MCP tool call requires approval, but approval policy
            // is never" (#13).
            "-c".into(),
            format!("mcp_servers.{MCP_SERVER}.default_tools_approval_mode=\"approve\""),
            // The message comes from stdin.
            "-".into(),
        ]);
        args
    }
}

/// A minimal call that only tells whether the quota admits a turn (data-model.md §8.4). It runs
/// outside every role's session, and `--ephemeral` keeps it out of the user's history. It uses
/// the roles' permission mode, so an environment that refuses one refuses both alike.
pub fn probe_args(permission: Permission) -> Vec<String> {
    let mut args: Vec<String> = ["exec", "--json", "--ephemeral"].map(String::from).to_vec();
    args.extend(permission_args(permission));
    args.extend(TRIMMED.map(String::from));
    args.extend(["-c", "model_reasoning_effort=\"low\"", "-"].map(String::from));
    args
}

/// What prints Codex's model catalog as JSON: the models, their reasoning efforts and the effort
/// each uses by default. It reads Codex's own cache and calls no model (0.08 s, codex-cli
/// 0.162.0-alpha.17.2). Unlike `exec`, it has no `--ignore-user-config`.
pub const CATALOG_ARGS: [&str; 2] = ["debug", "models"];

/// The models of [`CATALOG_ARGS`]'s output that Codex lists for choosing, in its order. `None`
/// when the output is not a catalog.
pub fn parse_catalog(json: &str) -> Option<Vec<Model>> {
    let catalog: Value = serde_json::from_str(json).ok()?;
    let mut listed: Vec<(i64, Model)> = catalog["models"]
        .as_array()?
        .iter()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| {
            let id = m["slug"].as_str()?.to_owned();
            let efforts = m["supported_reasoning_levels"].as_array().into_iter().flatten();
            let model = Model {
                label: m["display_name"].as_str().unwrap_or(&id).to_owned(),
                efforts: efforts.filter_map(|level| level["effort"].as_str().map(String::from)).collect(),
                default_effort: m["default_reasoning_level"].as_str().map(String::from),
                id,
            };
            Some((m["priority"].as_i64().unwrap_or(i64::MAX), model))
        })
        .collect();
    listed.sort_by_key(|(priority, _)| *priority);
    Some(listed.into_iter().map(|(_, model)| model).collect())
}

/// Capability trimming, the same for turns and quota checks: no user rules, config, connectors
/// or desktop control (harness-adapter.md §1.6).
const TRIMMED: [&str; 9] = [
    "--skip-git-repo-check",
    "--ignore-rules",
    "--ignore-user-config",
    "--disable",
    "apps",
    "--disable",
    "computer_use",
    "--disable",
    "browser_use",
];

/// The input of [`probe_args`].
pub const PROBE_INPUT: &str = "Reply with OK.";

/// Parses one line of `codex exec --json`. The format is not a stable promise, so unknown types
/// are kept, not rejected (harness-adapter.md §1.3 rule 7).
pub fn parse_line(line: &str) -> Option<Event> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return Some(Event::Unparsed(line.to_owned()));
    };
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    let event = match value["type"].as_str() {
        Some("thread.started") => match value["thread_id"].as_str() {
            Some(id) => Event::SessionStarted { native_id: id.to_owned() },
            None => Event::Unknown(value),
        },
        Some("turn.started") => Event::TurnStarted,
        Some("turn.completed") => Event::TurnCompleted { usage: value["usage"].clone() },
        Some("turn.failed") => Event::TurnFailed { message: text(&value["error"]["message"]) },
        Some("error") => Event::Error { message: text(&value["message"]) },
        Some(kind @ ("item.started" | "item.updated" | "item.completed")) => match item(&value["item"]) {
            Some(item) => match kind {
                "item.started" => Event::ItemStarted(item),
                "item.updated" => Event::ItemUpdated(item),
                _ => Event::ItemCompleted(item),
            },
            None => Event::Unknown(value),
        },
        _ => Event::Unknown(value),
    };
    Some(event)
}

fn item(raw: &Value) -> Option<Item> {
    let native_id = raw["id"].as_str()?.to_owned();
    let field = |name: &str| raw[name].clone();
    let (kind, content) = match raw["type"].as_str()? {
        "agent_message" => (ItemKind::AgentMessage, json!({ "text": field("text") })),
        "reasoning" => (ItemKind::Reasoning, json!({ "text": field("text") })),
        "command_execution" => (
            ItemKind::Command,
            json!({
                "command": field("command"),
                "output": field("aggregated_output"),
                "exit_code": field("exit_code"),
                "status": field("status"),
            }),
        ),
        "file_change" => (ItemKind::FileChange, json!({ "changes": field("changes"), "status": field("status") })),
        "mcp_tool_call" => {
            // Codex's own shapes stay here: the runtime reads only the two texts (#16).
            let result_text = raw["result"]["content"][0]["text"].as_str();
            let error_text = match &raw["error"] {
                Value::Null => None,
                error => {
                    Some(error["message"].as_str().or(error.as_str()).map_or_else(|| error.to_string(), str::to_owned))
                }
            };
            (
                ItemKind::McpCall,
                json!({
                    "server": field("server"),
                    "tool": field("tool"),
                    "arguments": field("arguments"),
                    "result": field("result"),
                    "error": field("error"),
                    "status": field("status"),
                    "result_text": result_text,
                    "error_text": error_text,
                }),
            )
        }
        "web_search" => (ItemKind::WebSearch, json!({ "query": field("query") })),
        "todo_list" => (ItemKind::TodoList, json!({ "items": field("items") })),
        "error" => (ItemKind::Error, json!({ "message": field("message") })),
        _ => (ItemKind::Other, raw.clone()),
    };
    Some(Item { native_id, kind, content })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user's skills are left out one by one; those Codex ships in `.system` stay
    /// (harness-adapter.md §1.6).
    #[test]
    fn a_turn_leaves_out_the_users_own_skills() {
        let home = tempfile::tempdir().unwrap();
        let skills = home.path().join("skills");
        for dir in ["mine", ".system/imagegen", "group/nested"] {
            std::fs::create_dir_all(skills.join(dir)).unwrap();
            std::fs::write(skills.join(dir).join("SKILL.md"), "---\nname: x\n---").unwrap();
        }
        std::fs::create_dir_all(skills.join("empty")).unwrap();
        let mut found = Vec::new();
        collect_skills(&skills, 0, &mut found);
        found.sort();
        assert_eq!(found, [skills.join("group/nested/SKILL.md"), skills.join("mine/SKILL.md")]);

        let mut args = TurnArgs {
            resume: None,
            model: None,
            reasoning_effort: None,
            permission: Permission::Full,
            developer_instructions: String::new(),
            mcp_url: String::new(),
            disabled_skills: vec![PathBuf::from(r"C:\Users\u\.codex\skills\mine\SKILL.md")],
        };
        let config = r#"skills.config=[{path="C:/Users/u/.codex/skills/mine/SKILL.md", enabled=false}]"#;
        assert!(args.to_args().contains(&config.to_owned()));
        args.disabled_skills.clear();
        assert!(!args.to_args().iter().any(|a| a.starts_with("skills.")), "no skills of the user's: nothing to pass");
    }
}
