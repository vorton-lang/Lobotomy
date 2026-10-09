//! The Claude Code CLI adapter: binary location, arguments and the events of
//! `claude -p --output-format stream-json` (harness-adapter.md §1.2–1.6, §1.9).

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::event::{Event, Item, ItemKind};
use crate::{MCP_SERVER, Permission};

/// Finds the Claude binary: `CLAUDE_BIN`, else `claude` from PATH (harness-adapter.md §1.5).
pub fn locate() -> PathBuf {
    std::env::var_os("CLAUDE_BIN").map_or_else(|| PathBuf::from("claude"), PathBuf::from)
}

/// An id for a new session. Claude takes it from the caller (`--session-id`) and requires a UUID:
/// these are 128 random bits laid out as a version 4 UUID.
pub fn new_session_id() -> String {
    let mut bytes = ulid::Ulid::generate().to_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// The session a turn runs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Session {
    /// The first turn of a native session, with the id it gets ([`new_session_id`]).
    New(String),
    Resume(String),
}

/// What one Claude turn needs. The message goes through stdin.
#[derive(Clone, Debug)]
pub struct TurnArgs {
    pub session: Session,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission: Permission,
    /// The role instructions. Claude records the system prompt on a conversation's first request
    /// and sends the record again until the conversation is compacted; after that it renders the
    /// prompt from the arguments of that launch. So every turn passes them (harness-adapter.md
    /// §1.3 rule 2).
    pub instructions: String,
    /// The file that holds [`mcp_config`] with this turn's URL. Its name carries the turn id, which
    /// tells the CLI's process apart on Linux (data-model.md §3.3).
    pub mcp_config: PathBuf,
}

/// The arguments of a permission mode (harness-adapter.md §1.9). Lobotomy's own tools never wait
/// for an approval, in either mode.
fn permission_args(permission: Permission) -> Vec<String> {
    let mode: &[&str] = match permission {
        Permission::Full => &["--dangerously-skip-permissions"],
        // Nobody answers a prompt in Lobotomy: what would prompt is refused.
        Permission::AutoReview => &["--permission-mode", "auto", "--permission-prompts", "none"],
    };
    let mut args: Vec<String> = mode.iter().map(|s| (*s).to_owned()).collect();
    args.extend(["--allowed-tools".into(), format!("mcp__{MCP_SERVER}")]);
    args
}

/// Whether the CLI's stderr says the environment does not allow the permission mode it was given.
/// Claude's wording for it is not known yet (harness-adapter.md §6), so no refusal is recognized.
pub fn permission_refused(_stderr: &str) -> bool {
    false
}

/// Tools that act outside Lobotomy's turns: schedules, remote triggers, notifications, messages to
/// other sessions, worktrees, background workflows (harness-adapter.md §1.6).
const DISALLOWED: &[&str] = &[
    "CronCreate",
    "CronDelete",
    "CronList",
    "ScheduleWakeup",
    "RemoteTrigger",
    "PushNotification",
    "SendMessage",
    "ListAgents",
    "EnterWorktree",
    "ExitWorktree",
    "DesignSync",
    "Workflow",
];

/// Capability trimming, the same for turns and quota checks: only Lobotomy's MCP server, none of
/// the tools above, and no settings of the user's own, whose hooks and permission rules assume a
/// person at the keyboard (harness-adapter.md §1.6, §1.9). `--mcp-config` takes several values, so
/// it comes after these.
fn trimmed() -> Vec<String> {
    vec![
        "--setting-sources".into(),
        String::new(),
        "--strict-mcp-config".into(),
        "--disallowed-tools".into(),
        DISALLOWED.join(","),
    ]
}

/// The MCP configuration of one turn: Lobotomy's server at the URL with the turn's token.
pub fn mcp_config(url: &str) -> String {
    json!({ "mcpServers": { MCP_SERVER: { "type": "http", "url": url } } }).to_string()
}

impl TurnArgs {
    pub fn to_args(&self) -> Vec<String> {
        let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"]
            .map(String::from)
            .to_vec();
        match &self.session {
            Session::New(id) => args.extend(["--session-id".into(), id.clone()]),
            Session::Resume(id) => args.extend(["--resume".into(), id.clone()]),
        }
        args.extend(permission_args(self.permission));
        if let Some(model) = &self.model {
            args.extend(["--model".into(), model.clone()]);
        }
        if let Some(effort) = &self.effort {
            args.extend(["--effort".into(), effort.clone()]);
        }
        args.extend(["--append-system-prompt".into(), self.instructions.clone()]);
        args.extend(trimmed());
        args.extend(["--mcp-config".into(), self.mcp_config.to_string_lossy().into_owned()]);
        args
    }
}

/// A minimal call that only tells whether the quota admits a turn (data-model.md §8.4). It keeps
/// no session, uses the smallest model, and the roles' permission mode, so an environment that
/// refuses one refuses both alike.
pub fn probe_args(permission: Permission) -> Vec<String> {
    let mut args: Vec<String> =
        ["-p", "--output-format", "stream-json", "--verbose", "--no-session-persistence", "--model", "haiku"]
            .map(String::from)
            .to_vec();
    args.extend(permission_args(permission));
    args.extend(trimmed());
    args
}

/// The input of [`probe_args`].
pub const PROBE_INPUT: &str = "Reply with OK.";

/// Turns the lines of `claude -p --output-format stream-json --verbose --include-partial-messages`
/// into events. Unlike Codex's output, Claude's needs state: a tool's result comes in a later
/// message than its call, and a block's text streams in before the block itself arrives.
///
/// The format is not a stable promise, so what the parser does not know is kept, not rejected
/// (harness-adapter.md §1.3 rule 7). Recorded from Claude Code 2.1.283.
#[derive(Debug, Default)]
pub struct Parser {
    /// The session's working directory, from `system/init`. File paths under it are shown
    /// relative to it.
    cwd: Option<String>,
    /// The message that partial messages stream into, and its open block.
    message: Option<String>,
    block: Option<u64>,
    /// The text streamed so far into the open block.
    text: String,
    /// Tool calls waiting for their results: their name and input, by tool use id.
    calls: HashMap<String, (String, Value)>,
    /// When a refused quota resets, once a `rate_limit_event` said so (Unix milliseconds).
    resets_at: Option<i64>,
}

/// The id of a message's block: Claude sends one block per `assistant` event, without its index.
fn block_id(message: &str, index: u64) -> String {
    format!("{message}:{index}")
}

impl Parser {
    pub fn parse_line(&mut self, line: &str) -> Vec<Event> {
        let line = line.trim();
        if line.is_empty() {
            return vec![];
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return vec![Event::Unparsed(line.to_owned())];
        };
        match value["type"].as_str() {
            Some("system") => self.system(value),
            Some("stream_event") => self.partial(&value["event"]),
            Some("assistant") => self.assistant(&value),
            Some("user") => self.user(&value),
            Some("rate_limit_event") => self.rate_limit(&value),
            Some("result") => self.result(&value),
            _ => vec![Event::Unknown(value)],
        }
    }

    fn system(&mut self, value: Value) -> Vec<Event> {
        match value["subtype"].as_str() {
            Some("init") => {
                self.cwd = value["cwd"].as_str().map(str::to_owned);
                match value["session_id"].as_str() {
                    Some(id) => vec![Event::SessionStarted { native_id: id.to_owned() }],
                    None => vec![Event::Unknown(value)],
                }
            }
            // Progress notices and compaction: nothing the transcript keeps yet.
            Some("status" | "thinking_tokens" | "compact_boundary") => vec![],
            _ => vec![Event::Unknown(value)],
        }
    }

    /// Partial messages only show the text of a block while it streams. The rest of them (tool
    /// input, thinking, the ends of blocks and messages) arrives whole in the `assistant` event.
    fn partial(&mut self, event: &Value) -> Vec<Event> {
        match event["type"].as_str() {
            Some("message_start") => {
                self.message = event["message"]["id"].as_str().map(str::to_owned);
                self.block = None;
            }
            Some("content_block_start") => {
                self.block = event["index"].as_u64();
                self.text.clear();
            }
            Some("content_block_delta") if event["delta"]["type"] == "text_delta" => {
                if let (Some(message), Some(index)) = (&self.message, self.block) {
                    self.text.push_str(event["delta"]["text"].as_str().unwrap_or_default());
                    let item = Item {
                        native_id: block_id(message, index),
                        kind: ItemKind::AgentMessage,
                        content: json!({ "text": self.text }),
                    };
                    return vec![Event::ItemUpdated(item)];
                }
            }
            _ => {}
        }
        vec![]
    }

    fn assistant(&mut self, value: &Value) -> Vec<Event> {
        // A subagent's own messages stay inside the tool call that started it.
        if !value["parent_tool_use_id"].is_null() {
            return vec![];
        }
        let message = &value["message"];
        let id = message["id"].as_str().unwrap_or_default();
        let blocks = message["content"].as_array().map(Vec::as_slice).unwrap_or_default();
        // An API error comes as a message Claude Code makes up itself, such as "You've hit your
        // session limit · resets 12:20am (Asia/Tokyo)" (data-model.md §8.3). It is no reply.
        if value["error"].is_string() || value["isApiErrorMessage"] == true {
            let text = blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n");
            let mut events = Vec::new();
            if value["error"] == "rate_limit" {
                events.push(Event::QuotaRejected { resets_at: self.resets_at, message: text.clone() });
            }
            let item = Item { native_id: id.to_owned(), kind: ItemKind::Error, content: json!({ "message": text }) };
            events.push(Event::ItemCompleted(item));
            return events;
        }
        let mut events = Vec::new();
        for (i, block) in blocks.iter().enumerate() {
            // The block arrives while it is the open block of the stream.
            let index = match (self.message.as_deref(), self.block) {
                (Some(open), Some(index)) if open == id && blocks.len() == 1 => index,
                _ => i as u64,
            };
            events.extend(self.block(id, index, block));
        }
        events
    }

    fn block(&mut self, message: &str, index: u64, block: &Value) -> Option<Event> {
        let completed =
            |kind, content| Event::ItemCompleted(Item { native_id: block_id(message, index), kind, content });
        match block["type"].as_str() {
            Some("text") => {
                let text = block["text"].as_str().unwrap_or_default();
                (!text.is_empty()).then(|| completed(ItemKind::AgentMessage, json!({ "text": text })))
            }
            // Claude Code may send thinking as a signature without its text.
            Some("thinking") => {
                let text = block["thinking"].as_str().unwrap_or_default();
                (!text.is_empty()).then(|| completed(ItemKind::Reasoning, json!({ "text": text })))
            }
            Some("redacted_thinking") => None,
            Some("tool_use") => {
                let (Some(call), Some(name)) = (block["id"].as_str(), block["name"].as_str()) else {
                    return Some(Event::Unknown(block.clone()));
                };
                let input = block["input"].clone();
                let item = self.tool_item(call, name, &input, None);
                self.calls.insert(call.to_owned(), (name.to_owned(), input));
                Some(Event::ItemStarted(item))
            }
            _ => Some(completed(ItemKind::Other, block.clone())),
        }
    }

    fn user(&mut self, value: &Value) -> Vec<Event> {
        if !value["parent_tool_use_id"].is_null() {
            return vec![];
        }
        // The turn's input comes back as a string; tool results come as blocks.
        let Some(blocks) = value["message"]["content"].as_array() else { return vec![] };
        let results: Vec<&Value> = blocks.iter().filter(|b| b["type"] == "tool_result").collect();
        let mut events = Vec::new();
        for block in &results {
            let Some((name, input)) = block["tool_use_id"].as_str().and_then(|id| self.calls.remove(id)) else {
                events.push(Event::Unknown((*block).clone()));
                continue;
            };
            let call = block["tool_use_id"].as_str().unwrap_or_default();
            let result = ToolResult {
                text: result_text(&block["content"]),
                is_error: block["is_error"].as_bool().unwrap_or(false),
                content: block["content"].clone(),
                // Claude's own record of the result; it stands for the only result of the message.
                record: if results.len() == 1 { value["tool_use_result"].clone() } else { Value::Null },
            };
            events.push(Event::ItemCompleted(self.tool_item(call, &name, &input, Some(&result))));
        }
        events
    }

    /// A tool call as an item of the kind it belongs to. Without a result, it is still running.
    fn tool_item(&self, call: &str, name: &str, input: &Value, result: Option<&ToolResult>) -> Item {
        let status = match result {
            None => "in_progress",
            Some(r) if r.is_error => "failed",
            Some(_) => "completed",
        };
        let output = result.map(|r| r.text.clone());
        let (kind, content) = if let Some(qualified) = name.strip_prefix("mcp__") {
            let (server, tool) = qualified.split_once("__").unwrap_or((qualified, ""));
            let (result_text, error_text) = match result {
                Some(r) if r.is_error => (None, Some(r.text.clone())),
                Some(r) => (Some(r.text.clone()), None),
                None => (None, None),
            };
            let content = json!({
                "server": server,
                "tool": tool,
                "arguments": input,
                "result": result.filter(|r| !r.is_error).map(|r| &r.content),
                "error": result.filter(|r| r.is_error).map(|r| &r.content),
                "status": status,
                "result_text": result_text,
                "error_text": error_text,
            });
            (ItemKind::McpCall, content)
        } else {
            match name {
                "Bash" | "PowerShell" => (
                    ItemKind::Command,
                    json!({
                        "command": input["command"],
                        "output": output,
                        "exit_code": result.and_then(exit_code),
                        "status": status,
                    }),
                ),
                "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
                    let path = input["file_path"].as_str().or(input["notebook_path"].as_str()).unwrap_or_default();
                    let created = name == "Write" && result.is_some_and(|r| r.record["type"] == "create");
                    let change = json!({ "path": self.relative(path), "kind": if created { "add" } else { "update" } });
                    (ItemKind::FileChange, json!({ "changes": [change], "status": status }))
                }
                "WebSearch" => (ItemKind::WebSearch, json!({ "query": input["query"] })),
                "TodoWrite" => {
                    let items: Vec<Value> = input["todos"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                        .iter()
                        .map(|t| json!({ "text": t["content"], "completed": t["status"] == "completed" }))
                        .collect();
                    (ItemKind::TodoList, json!({ "items": items }))
                }
                _ => {
                    // Paths in the working directory are shown relative to it, as file changes are.
                    let mut input = input.clone();
                    if let Value::Object(fields) = &mut input {
                        for key in ["file_path", "notebook_path", "path"] {
                            if let Some(Value::String(path)) = fields.get_mut(key) {
                                *path = self.relative(path);
                            }
                        }
                    }
                    (ItemKind::ToolCall, json!({ "tool": name, "input": input, "output": output, "status": status }))
                }
            }
        };
        Item { native_id: call.to_owned(), kind, content }
    }

    /// A quota refusal. The other states, allowed and allowed with a warning, change nothing here.
    fn rate_limit(&mut self, value: &Value) -> Vec<Event> {
        let info = &value["rate_limit_info"];
        if info["status"] != "rejected" {
            return vec![];
        }
        self.resets_at = info["resetsAt"].as_i64().map(|seconds| seconds * 1000);
        let window = info["rateLimitType"].as_str().unwrap_or("unknown");
        vec![Event::QuotaRejected {
            resets_at: self.resets_at, message: format!("Claude 的额度已用完（{window}）")
        }]
    }

    /// The end of the turn. A 429 is a refused quota, whatever else the turn said.
    fn result(&self, value: &Value) -> Vec<Event> {
        if value["subtype"] == "success" && value["is_error"] != true {
            return vec![Event::TurnCompleted { usage: value["usage"].clone() }];
        }
        let message = match value["result"].as_str().filter(|text| !text.is_empty()) {
            Some(text) => text.to_owned(),
            None => {
                let subtype = value["subtype"].as_str().unwrap_or("error");
                match value["api_error_status"].as_i64() {
                    Some(status) => format!("{subtype}（API 状态 {status}）"),
                    None => subtype.to_owned(),
                }
            }
        };
        let mut events = Vec::new();
        if value["api_error_status"] == 429 {
            events.push(Event::QuotaRejected { resets_at: self.resets_at, message: message.clone() });
        }
        events.push(Event::TurnFailed { message });
        events
    }

    /// A path under the session's working directory, relative to it; any other path as it is.
    fn relative(&self, path: &str) -> String {
        let Some(cwd) = &self.cwd else { return path.to_owned() };
        match path.strip_prefix(cwd.as_str()) {
            Some(rest) if rest.starts_with(['/', '\\']) => rest[1..].to_owned(),
            _ => path.to_owned(),
        }
    }
}

struct ToolResult {
    /// What the model read: the text of the result.
    text: String,
    is_error: bool,
    content: Value,
    /// `tool_use_result`, Claude's structured record of the result.
    record: Value,
}

/// A result's text: the string itself, or its text blocks joined. A result without text, such as
/// the tools a search found, as its JSON.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks.iter().filter_map(|b| b["text"].as_str()).collect();
            if texts.is_empty() { content.to_string() } else { texts.join("\n") }
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A command's exit code: 0 when it succeeded; when it failed, the code Claude puts first in the
/// result ("Exit code 3"), if it does.
fn exit_code(result: &ToolResult) -> Option<i64> {
    if !result.is_error {
        return Some(0);
    }
    let rest = result.text.strip_prefix("Exit code ")?;
    rest.split(|c: char| !c.is_ascii_digit() && c != '-').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(lines: &str) -> Vec<Event> {
        let mut parser = Parser::default();
        lines.lines().flat_map(|line| parser.parse_line(line)).collect()
    }

    fn completed(events: &[Event], id: &str) -> Item {
        events
            .iter()
            .find_map(|e| match e {
                Event::ItemCompleted(item) if item.native_id == id => Some(item.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no completed item {id}"))
    }

    /// A real turn of Claude Code 2.1.283 with haiku (2026-10-09): two commands, one failing, a
    /// file written and read, two tasks, and text before and after. Shortened: thinking signatures
    /// and streamed tool input are left out, and the working directory is `C:\work`.
    const TURN: &str = include_str!("../tests/fixtures/claude-turn.jsonl");

    #[test]
    fn a_real_turn_parses_into_events() {
        let events = parse(TURN);
        assert_eq!(events[0], Event::SessionStarted { native_id: "ee6e6082-bdf3-40ef-84c7-b3b6b13a6b57".into() });
        assert!(
            !events.iter().any(|e| matches!(e, Event::Unknown(_) | Event::Unparsed(_))),
            "{:?}",
            events.iter().filter(|e| matches!(e, Event::Unknown(_) | Event::Unparsed(_))).collect::<Vec<_>>()
        );
        // Thinking came without its text: no reasoning items.
        assert!(!events.iter().any(|e| matches!(e, Event::ItemCompleted(i) if i.kind == ItemKind::Reasoning)));

        let first = "msg_011Cfrm9oUtFisRLUMW9C3qq:1";
        let streamed: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::ItemUpdated(i) if i.native_id == first => i.content["text"].as_str(),
                _ => None,
            })
            .collect();
        assert!(streamed.len() > 3 && streamed.windows(2).all(|w| w[1].starts_with(w[0])), "{streamed:?}");
        let text = completed(&events, first);
        assert_eq!(text.content["text"], "我会按顺序执行这些步骤。先从第一个命令开始。");
        assert_eq!(streamed.last(), text.content["text"].as_str().as_ref());

        let echo = completed(&events, "toolu_014hZdLErCq7HMJLZRrPrwx4");
        assert_eq!(echo.kind, ItemKind::Command);
        assert_eq!(
            (&echo.content["command"], &echo.content["output"], &echo.content["exit_code"]),
            (&json!("echo probe-ok"), &json!("probe-ok"), &json!(0))
        );
        let exit = completed(&events, "toolu_019bSLVBEQPdh21xyKxPQ19C");
        assert_eq!((&exit.content["exit_code"], &exit.content["status"]), (&json!(3), &json!("failed")));

        let write = completed(&events, "toolu_013DneqFbyD4ha2jxirbepNA");
        assert_eq!(
            (write.kind, &write.content["changes"]),
            (ItemKind::FileChange, &json!([{ "path": "notes.txt", "kind": "add" }]))
        );
        let read = completed(&events, "toolu_015y4HgVPcSZATWRgTioyVTH");
        assert_eq!(
            (read.kind, &read.content["tool"], &read.content["output"]),
            (ItemKind::ToolCall, &json!("Read"), &json!("1\thello"))
        );
        assert_eq!(read.content["input"]["file_path"], "notes.txt", "relative to the working directory");

        assert!(matches!(events.last(), Some(Event::TurnCompleted { usage }) if usage["output_tokens"] == 2699));
    }

    #[test]
    fn a_call_is_started_when_it_arrives_and_completed_with_its_result() {
        let events = parse(TURN);
        let started = events
            .iter()
            .position(|e| matches!(e, Event::ItemStarted(i) if i.native_id == "toolu_014hZdLErCq7HMJLZRrPrwx4"));
        let done = events
            .iter()
            .position(|e| matches!(e, Event::ItemCompleted(i) if i.native_id == "toolu_014hZdLErCq7HMJLZRrPrwx4"));
        assert!(matches!((started, done), (Some(s), Some(d)) if s < d));
    }

    const ORG_REPORT: &str = r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"tool_use","id":"toolu_1","name":"mcp__lobotomy__org_report","input":{"title":"t","body":"b","status":"done"}}]},"parent_tool_use_id":null}"#;

    /// Lobotomy's tools fill the fields the runtime reads, as Codex's adapter does (#16).
    #[test]
    fn a_lobotomy_tool_call_says_what_came_back() {
        let ok = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"recorded (cmd_1)"}]}]},"parent_tool_use_id":null}"#;
        let call = completed(&parse(&format!("{ORG_REPORT}\n{ok}")), "toolu_1");
        assert_eq!(
            (call.kind, &call.content["server"], &call.content["tool"]),
            (ItemKind::McpCall, &json!("lobotomy"), &json!("org_report"))
        );
        assert_eq!(
            (&call.content["result_text"], &call.content["error_text"]),
            (&json!("recorded (cmd_1)"), &Value::Null)
        );
        assert_eq!(call.content["arguments"]["status"], "done");

        let refused = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"Permission to use mcp__lobotomy__org_report has been denied.","is_error":true}]},"parent_tool_use_id":null}"#;
        let call = completed(&parse(&format!("{ORG_REPORT}\n{refused}")), "toolu_1");
        assert_eq!(call.content["error_text"], "Permission to use mcp__lobotomy__org_report has been denied.");
        assert_eq!((&call.content["result_text"], &call.content["status"]), (&Value::Null, &json!("failed")));
    }

    #[test]
    fn a_subagents_own_messages_stay_inside_its_call() {
        let inner = r#"{"type":"assistant","message":{"id":"msg_2","content":[{"type":"text","text":"inside"}]},"parent_tool_use_id":"toolu_9"}"#;
        assert!(parse(inner).is_empty());
    }

    /// What Claude Code 2.1.283 wrote when the session limit was hit, in the interactive CLI
    /// (2026-10-01, data-model.md §8.3): a message it makes up, with the error and status at the
    /// top level. In `-p` the same signals are expected on the stream's `assistant` and `result`
    /// events; the parser takes any of them, with the reset time from a `rate_limit_event`.
    #[test]
    fn a_session_limit_is_a_refused_quota() {
        let limit = r#"{"type":"assistant","message":{"id":"7fd4a852-9929-46f1-9fed-154e90aa0d6e","model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"You've hit your session limit · resets 12:20am (Asia/Tokyo)"}]},"parent_tool_use_id":null,"error":"rate_limit","isApiErrorMessage":true,"apiErrorStatus":429}"#;
        let refused = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1790868000,"rateLimitType":"five_hour"}}"#;
        let end = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":429,"result":"You've hit your session limit · resets 12:20am (Asia/Tokyo)"}"#;
        let said = "You've hit your session limit · resets 12:20am (Asia/Tokyo)";

        let events = parse(&format!("{refused}\n{limit}\n{end}"));
        let quota: Vec<_> = events.iter().filter(|e| matches!(e, Event::QuotaRejected { .. })).collect();
        assert_eq!(quota.len(), 3, "{events:?}");
        assert!(quota.iter().all(|e| matches!(e, Event::QuotaRejected { resets_at: Some(1_790_868_000_000), .. })));
        let error = completed(&events, "7fd4a852-9929-46f1-9fed-154e90aa0d6e");
        assert_eq!((error.kind, &error.content["message"]), (ItemKind::Error, &json!(said)));
        assert!(!events.iter().any(|e| matches!(e, Event::ItemCompleted(i) if i.kind == ItemKind::AgentMessage)));
        assert_eq!(events.last(), Some(&Event::TurnFailed { message: said.into() }));

        // Without a rate_limit_event, the refusal is still known; only the reset time is not.
        let events = parse(&format!("{limit}\n{end}"));
        assert_eq!(events[0], Event::QuotaRejected { resets_at: None, message: said.into() });
    }

    #[test]
    fn a_rejected_quota_and_a_failed_turn_say_so() {
        let rejected = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1792076400,"rateLimitType":"five_hour"}}"#;
        assert_eq!(
            parse(rejected),
            vec![Event::QuotaRejected {
                resets_at: Some(1_792_076_400_000),
                message: "Claude 的额度已用完（five_hour）".into()
            }]
        );
        let allowed =
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1792076400}}"#;
        assert!(parse(allowed).is_empty());
        let failed = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"api_error_status":529}"#;
        assert_eq!(parse(failed), vec![Event::TurnFailed { message: "error_during_execution（API 状态 529）".into() }]);
        let said = r#"{"type":"result","subtype":"success","is_error":true,"result":"API Error: overloaded"}"#;
        assert_eq!(parse(said), vec![Event::TurnFailed { message: "API Error: overloaded".into() }]);
    }

    #[test]
    fn unknown_and_broken_lines_are_kept() {
        assert!(matches!(parse(r#"{"type":"someday"}"#)[..], [Event::Unknown(_)]));
        assert!(matches!(parse(r#"{"type":"system","subtype":"someday"}"#)[..], [Event::Unknown(_)]));
        assert_eq!(parse("not json"), vec![Event::Unparsed("not json".into())]);
        assert!(parse("  ").is_empty());
    }

    fn args(session: Session, permission: Permission) -> Vec<String> {
        TurnArgs {
            session,
            model: Some("sonnet".into()),
            effort: None,
            permission,
            instructions: "你是 Malkuth。".into(),
            mcp_config: PathBuf::from("C:/data/turns/turn_1.mcp.json"),
        }
        .to_args()
    }

    #[test]
    fn a_turn_names_its_session_and_ends_with_the_mcp_config() {
        let first = args(Session::New("0f0e0d0c-0b0a-4908-8706-050403020100".into()), Permission::Full);
        let at = first.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(first[at + 1], "0f0e0d0c-0b0a-4908-8706-050403020100");
        assert!(!first.contains(&"--resume".to_owned()));
        assert_eq!(&first[first.len() - 2..], ["--mcp-config", "C:/data/turns/turn_1.mcp.json"]);
        let resumed = args(Session::Resume("abc".into()), Permission::Full);
        let at = resumed.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(resumed[at + 1], "abc");
        for flag in ["--dangerously-skip-permissions", "--strict-mcp-config", "--append-system-prompt"] {
            assert!(first.contains(&flag.to_owned()), "{flag} missing");
        }
        let sources = first.iter().position(|a| a == "--setting-sources").unwrap();
        assert_eq!(first[sources + 1], "", "no settings of the user's own");
    }

    #[test]
    fn auto_review_refuses_what_would_prompt_and_allows_lobotomys_tools() {
        let auto = args(Session::Resume("abc".into()), Permission::AutoReview);
        let pairs: Vec<(&str, &str)> = auto.windows(2).map(|w| (w[0].as_str(), w[1].as_str())).collect();
        for pair in
            [("--permission-mode", "auto"), ("--permission-prompts", "none"), ("--allowed-tools", "mcp__lobotomy")]
        {
            assert!(pairs.contains(&pair), "{pair:?} missing from {auto:?}");
        }
        assert!(!auto.contains(&"--dangerously-skip-permissions".to_owned()));
        assert!(probe_args(Permission::AutoReview).contains(&"--no-session-persistence".to_owned()));
    }

    #[test]
    fn session_ids_are_version_4_uuids() {
        let id = new_session_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4", "{id}");
        assert!(matches!(id.as_bytes()[19], b'8' | b'9' | b'a' | b'b'), "{id}");
        assert_ne!(new_session_id(), id);
    }

    #[test]
    fn the_mcp_config_names_lobotomys_server() {
        let config: Value = serde_json::from_str(&mcp_config("http://127.0.0.1:1/mcp/tok_1")).unwrap();
        assert_eq!(config["mcpServers"]["lobotomy"], json!({ "type": "http", "url": "http://127.0.0.1:1/mcp/tok_1" }));
    }
}
