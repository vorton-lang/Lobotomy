//! Harness adapters and the platform layer (notes/harness-adapter.md): Codex, and Claude from M2.

pub mod claude;
pub mod codex;
pub mod event;
pub mod process;

/// The name of Lobotomy's MCP server in every harness's configuration. Calls to its tools are
/// recognized by it (data-model.md §7.2).
pub const MCP_SERVER: &str = "lobotomy";

/// A harness Lobotomy can run roles on. Code that differs by harness matches on this, so adding
/// Claude (M3) makes the compiler point at every such place (#16). The adapter interface itself
/// waits until Claude's real output is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Harness {
    Codex,
}

impl Harness {
    pub const ALL: &[Harness] = &[Harness::Codex];

    /// The name roles and settings store.
    pub fn as_str(self) -> &'static str {
        match self {
            Harness::Codex => "codex",
        }
    }

    /// `None` for a harness this build cannot run, such as `claude` before M3.
    pub fn parse(s: &str) -> Option<Self> {
        Harness::ALL.iter().copied().find(|h| h.as_str() == s)
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
