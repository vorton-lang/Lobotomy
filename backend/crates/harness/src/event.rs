//! Harness-neutral events, parsed from a CLI's JSON output (harness-adapter.md §1.3 rule 7).

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Event {
    /// The harness's own session id. Codex reports it at the start of every turn.
    SessionStarted {
        native_id: String,
    },
    TurnStarted,
    ItemStarted(Item),
    ItemUpdated(Item),
    ItemCompleted(Item),
    /// The turn ended normally. `usage` is the harness's own record, kept as is.
    TurnCompleted {
        usage: Value,
    },
    TurnFailed {
        message: String,
    },
    /// The harness refused the turn for its quota. `resets_at` is in Unix milliseconds, when the
    /// harness said (data-model.md §8.3).
    QuotaRejected {
        resets_at: Option<i64>,
        message: String,
    },
    /// An error outside an item, such as a lost connection the harness retries.
    Error {
        message: String,
    },
    /// Valid JSON of a type the adapter does not know. Kept so the raw output can be inspected.
    Unknown(Value),
    /// A line that is not JSON.
    Unparsed(String),
}

/// One unit of the transcript: a message, a command, a file change, a tool call
/// (data-model.md §7.2).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Item {
    /// The harness's id, unique within the turn.
    pub native_id: String,
    pub kind: ItemKind,
    /// The kind's fields, normalized across harnesses. Unknown kinds keep the raw item.
    pub content: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// `text`
    AgentMessage,
    /// `text`
    Reasoning,
    /// `command`, `output`, `exit_code`, `status`
    Command,
    /// `changes`: `[{path, kind}]`, `status`
    FileChange,
    /// `server`, `tool`, `arguments`, `status`; `result_text`, the first text the tool returned,
    /// and `error_text`, why the call failed, which every adapter fills; `result` and `error` as
    /// the harness gave them, for display only.
    McpCall,
    /// `query`
    WebSearch,
    /// `items`: `[{text, completed}]`
    TodoList,
    /// A tool of the harness's own that none of the kinds above covers, such as reading or
    /// searching files: `tool`, `input`, `output`, `status`.
    ToolCall,
    /// `message`
    Error,
    Other,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::AgentMessage => "agent_message",
            ItemKind::Reasoning => "reasoning",
            ItemKind::Command => "command",
            ItemKind::FileChange => "file_change",
            ItemKind::McpCall => "mcp_call",
            ItemKind::WebSearch => "web_search",
            ItemKind::TodoList => "todo_list",
            ItemKind::ToolCall => "tool_call",
            ItemKind::Error => "error",
            ItemKind::Other => "other",
        }
    }
}
