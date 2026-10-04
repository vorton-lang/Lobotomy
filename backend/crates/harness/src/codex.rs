//! The Codex CLI adapter: binary location, arguments and `exec --json` events
//! (harness-adapter.md §1.2, §1.5, §1.6).

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::Permission;
use crate::event::{Event, Item, ItemKind};

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
        args.extend(
            [
                "--skip-git-repo-check",
                // Capability trimming: no user rules, config, connectors or desktop control
                // (harness-adapter.md §1.6).
                "--ignore-rules",
                "--ignore-user-config",
                "--disable",
                "apps",
                "--disable",
                "computer_use",
                "--disable",
                "browser_use",
            ]
            .map(String::from),
        );
        if let Some(model) = &self.model {
            args.extend(["-m".into(), model.clone()]);
        }
        if let Some(effort) = &self.reasoning_effort {
            args.extend(["-c".into(), format!("model_reasoning_effort={}", toml_string(effort))]);
        }
        args.extend([
            "-c".into(),
            format!("developer_instructions={}", toml_string(&self.developer_instructions)),
            "-c".into(),
            format!("mcp_servers.lobotomy.url={}", toml_string(&self.mcp_url)),
            // Lobotomy's own tools never wait for an approval. Outside full access, Codex would
            // otherwise refuse org_report: "MCP tool call requires approval, but approval policy
            // is never" (#13).
            "-c".into(),
            "mcp_servers.lobotomy.default_tools_approval_mode=\"approve\"".into(),
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
    let mut args: Vec<String> = ["exec", "--json"].map(String::from).to_vec();
    args.extend(permission_args(permission));
    args.extend(
        [
            "--ephemeral",
            "--skip-git-repo-check",
            "--ignore-rules",
            "--ignore-user-config",
            "--disable",
            "apps",
            "--disable",
            "computer_use",
            "--disable",
            "browser_use",
            "-c",
            "model_reasoning_effort=\"low\"",
            "-",
        ]
        .map(String::from),
    );
    args
}

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
        "mcp_tool_call" => (
            ItemKind::McpCall,
            json!({
                "server": field("server"),
                "tool": field("tool"),
                "arguments": field("arguments"),
                "result": field("result"),
                "error": field("error"),
                "status": field("status"),
            }),
        ),
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

    /// A real turn of Codex CLI 0.159.2 (2026-10-04), shortened.
    const SAMPLE: &str = r#"{"type":"thread.started","thread_id":"01a104d4-07c5-7063-9a66-7c93fd2cdd3f"}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"I'll run the command.\n"}}
{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"pwsh -Command 'echo hello-sample'","aggregated_output":"","exit_code":null,"status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"pwsh -Command 'echo hello-sample'","aggregated_output":"hello-sample\r\n","exit_code":0,"status":"completed"}}
{"type":"item.completed","item":{"id":"item_3","type":"mcp_tool_call","server":"lobotomy","tool":"org_report","arguments":{"title":"sample","body":"made notes.txt","status":"done"},"result":{"content":[{"type":"text","text":"recorded"}],"structured_content":null},"error":null,"status":"completed"}}
{"type":"turn.completed","usage":{"input_tokens":61648,"cached_input_tokens":51840,"output_tokens":169}}"#;

    fn events() -> Vec<Event> {
        SAMPLE.lines().filter_map(parse_line).collect()
    }

    #[test]
    fn a_real_turn_parses_into_events() {
        let events = events();
        assert_eq!(events[0], Event::SessionStarted { native_id: "01a104d4-07c5-7063-9a66-7c93fd2cdd3f".into() });
        assert_eq!(events[1], Event::TurnStarted);
        let Event::ItemCompleted(message) = &events[2] else { panic!("{:?}", events[2]) };
        assert_eq!(
            (message.kind, message.content["text"].as_str()),
            (ItemKind::AgentMessage, Some("I'll run the command.\n"))
        );
        let Event::ItemStarted(started) = &events[3] else { panic!("{:?}", events[3]) };
        assert_eq!((started.kind, started.content["status"].as_str()), (ItemKind::Command, Some("in_progress")));
        let Event::ItemCompleted(done) = &events[4] else { panic!("{:?}", events[4]) };
        assert_eq!(done.content["output"], "hello-sample\r\n");
        assert_eq!(done.content["exit_code"], 0);
        let Event::ItemCompleted(call) = &events[5] else { panic!("{:?}", events[5]) };
        assert_eq!((call.kind, call.content["tool"].as_str()), (ItemKind::McpCall, Some("org_report")));
        assert!(matches!(&events[6], Event::TurnCompleted { usage } if usage["output_tokens"] == 169));
    }

    #[test]
    fn unknown_and_broken_lines_are_kept() {
        assert!(matches!(parse_line(r#"{"type":"turn.someday"}"#), Some(Event::Unknown(_))));
        assert!(matches!(
            parse_line(r#"{"type":"item.completed","item":{"id":"i","type":"hologram","x":1}}"#),
            Some(Event::ItemCompleted(Item { kind: ItemKind::Other, .. }))
        ));
        assert_eq!(parse_line("not json"), Some(Event::Unparsed("not json".into())));
        assert_eq!(parse_line("   "), None);
    }

    #[test]
    fn failures_carry_their_message() {
        assert_eq!(
            parse_line(r#"{"type":"turn.failed","error":{"message":"boom"}}"#),
            Some(Event::TurnFailed { message: "boom".into() })
        );
        assert_eq!(
            parse_line(r#"{"type":"error","message":"retrying"}"#),
            Some(Event::Error { message: "retrying".into() })
        );
    }

    fn turn(resume: Option<&str>, permission: Permission) -> Vec<String> {
        TurnArgs {
            resume: resume.map(str::to_owned),
            model: None,
            reasoning_effort: Some("low".into()),
            permission,
            developer_instructions: "你是 Malkuth。\n用 \"org_report\" 汇报。".into(),
            mcp_url: "http://127.0.0.1:1/mcp/tok_1".into(),
        }
        .to_args()
    }

    #[test]
    fn resume_puts_the_session_id_after_exec_resume_and_reads_stdin() {
        let args = turn(Some("thread-1"), Permission::Full);
        assert_eq!(&args[..3], ["exec", "resume", "thread-1"]);
        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert!(args.contains(&r#"developer_instructions="你是 Malkuth。\n用 \"org_report\" 汇报。""#.to_owned()));
        assert!(args.contains(&r#"mcp_servers.lobotomy.url="http://127.0.0.1:1/mcp/tok_1""#.to_owned()));
        assert!(args.contains(&r#"model_reasoning_effort="low""#.to_owned()));
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_owned()));
    }

    /// `--approve-for-me` is refused after `exec resume`; the settings it stands for are not.
    #[test]
    fn auto_review_passes_settings_that_work_for_first_and_resumed_turns() {
        for resume in [None, Some("thread-1")] {
            let args = turn(resume, Permission::AutoReview);
            let pairs: Vec<String> = args.windows(2).filter(|w| w[0] == "-c").map(|w| w[1].clone()).collect();
            for setting in [
                r#"approval_policy="on-request""#,
                r#"approvals_reviewer="auto_review""#,
                r#"sandbox_mode="workspace-write""#,
                r#"mcp_servers.lobotomy.default_tools_approval_mode="approve""#,
            ] {
                assert!(pairs.contains(&setting.to_owned()), "{setting} missing from {args:?}");
            }
            assert!(!args.iter().any(|a| a.starts_with("--dangerously") || a == "--approve-for-me"), "{args:?}");
        }
        let probe = probe_args(Permission::AutoReview);
        assert!(probe.contains(&r#"approvals_reviewer="auto_review""#.to_owned()));
        assert!(!probe.contains(&"--dangerously-bypass-approvals-and-sandbox".to_owned()));
    }

    #[test]
    fn a_requirements_refusal_is_recognized() {
        let stderr = "Error: `approval_policy = \"never\"` cannot be used because requirements do not allow \
                      `sandbox_mode = \"danger-full-access\"`; Codex would fall back to read-only permissions";
        assert!(permission_refused(stderr));
        assert!(!permission_refused("Error: mcp_servers.lobotomy.url = x is not allowed here"));
    }
}
