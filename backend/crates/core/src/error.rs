#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The command was refused: a permission or precondition check failed. Nothing was written.
    #[error("rejected ({code}): {message}")]
    Rejected { code: &'static str, message: String },
    /// Stored state breaks a rule the code relies on: a corrupt row or a bug. Unlike a rejection,
    /// waiting does not make it go away, so callers that treat rejections as "not yet" must not
    /// treat this one so (#16).
    #[error("broken invariant: {0}")]
    Invariant(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn rejected(code: &'static str, message: impl Into<String>) -> Self {
        Error::Rejected { code, message: message.into() }
    }

    pub fn invariant(message: impl Into<String>) -> Self {
        Error::Invariant(message.into())
    }

    /// The rejection code, if this is a rejection.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Error::Rejected { code, .. } => Some(code),
            _ => None,
        }
    }
}
