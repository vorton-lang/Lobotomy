//! The Claude Code CLI adapter: binary location, arguments and the events of
//! `claude -p --output-format stream-json` (harness-adapter.md §1.2–1.6, §1.9).

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::event::{Event, Item, ItemKind};
use crate::{MCP_SERVER, Model, Permission};

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

/// The efforts `--effort` takes (Claude Code 2.1.283 help). Haiku accepts them too (2026-10-13).
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The models the user can choose: the aliases `--model` takes for the latest model of each
/// family. Claude Code has no command that lists them; its help names `fable`, `opus` and
/// `sonnet`, and `haiku` is what the quota check uses.
pub fn models() -> Vec<Model> {
    [("fable", "Fable"), ("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku")]
        .into_iter()
        .map(|(id, label)| Model {
            id: id.into(),
            label: label.into(),
            efforts: EFFORTS.map(String::from).to_vec(),
            default_effort: None,
        })
        .collect()
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
