//! Harness adapters and the platform layer (notes/harness-adapter.md): Codex, and Claude from M2.

pub mod claude;
pub mod codex;
pub mod event;
pub mod process;

/// The name of Lobotomy's MCP server in every harness's configuration. Calls to its tools are
/// recognized by it (data-model.md §7.2).
pub const MCP_SERVER: &str = "lobotomy";

/// A harness Lobotomy can run roles on. Code that differs by harness matches on this, so adding a
/// harness makes the compiler point at every such place (#16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Harness {
    Claude,
    Codex,
}

impl Harness {
    pub const ALL: &[Harness] = &[Harness::Claude, Harness::Codex];

    /// The name roles and settings store.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
        }
    }

    /// `None` for a harness this build cannot run.
    pub fn parse(s: &str) -> Option<Self> {
        Harness::ALL.iter().copied().find(|h| h.as_str() == s)
    }

    /// The product's name, in what the user reads.
    pub fn label(self) -> &'static str {
        match self {
            Harness::Claude => "Claude",
            Harness::Codex => "Codex",
        }
    }
}

/// Reads a harness's output one line at a time. Claude's parser keeps state between lines;
/// Codex's does not (harness-adapter.md §1.4).
pub enum Output {
    Claude(claude::Parser),
    Codex,
}

impl Output {
    pub fn new(harness: Harness) -> Self {
        match harness {
            Harness::Claude => Output::Claude(claude::Parser::default()),
            Harness::Codex => Output::Codex,
        }
    }

    pub fn parse_line(&mut self, line: &str) -> Vec<event::Event> {
        match self {
            Output::Claude(parser) => parser.parse_line(line),
            Output::Codex => codex::parse_line(line).into_iter().collect(),
        }
    }
}

/// What a role's CLI may do without asking (harness-adapter.md §1.9). Nobody answers a
/// permission prompt in Lobotomy, so no mode asks the user. Each harness has its own setting:
/// an environment can allow one harness full access and not the other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Permission {
    /// No approvals and no sandbox.
    #[default]
    Full,
    /// A sandbox; what goes beyond it is reviewed by the harness's automatic reviewer. For
    /// environments whose administrators do not allow full access.
    AutoReview,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Full => "full",
            Permission::AutoReview => "auto_review",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "full" => Some(Permission::Full),
            "auto_review" => Some(Permission::AutoReview),
            _ => None,
        }
    }
}
