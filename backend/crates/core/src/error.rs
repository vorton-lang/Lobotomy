#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The command was refused: a permission or precondition check failed. Nothing was written.
    #[error("rejected ({code}): {message}")]
    Rejected { code: &'static str, message: String },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn rejected(code: &'static str, message: impl Into<String>) -> Self {
        Error::Rejected { code, message: message.into() }
    }

    /// The rejection code, if this is a rejection.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Error::Rejected { code, .. } => Some(code),
            _ => None,
        }
    }
}
