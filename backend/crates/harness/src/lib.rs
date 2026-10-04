//! Harness adapters and the platform layer (notes/harness-adapter.md). M1 has the Codex adapter;
//! Claude arrives with M3.

/// The harnesses Lobotomy can run roles on.
pub const HARNESSES: &[&str] = &["codex"];

pub mod codex;
pub mod event;
pub mod process;

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
