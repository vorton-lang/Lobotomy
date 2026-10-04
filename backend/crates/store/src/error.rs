use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("git {args}: {message}")]
    Git { args: String, message: String },
    #[error("jj: {0}")]
    Jj(String),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// Checking out would have written over a file the store does not track, so the directory does
    /// not equal its target. The caller rebuilds it in a fresh generation (data-model.md §5).
    #[error("{0} files could not be written because untracked files are in the way")]
    Blocked(u32),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn jj(e: impl std::fmt::Display) -> Self {
        Error::Jj(e.to_string())
    }

    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.into();
        move |source| Error::Io { path, source }
    }
}
