use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeskError {
    #[error("RINEX parse error at {location}: {message}")]
    Parse { location: String, message: String },
    #[error("unknown project {0}")]
    UnknownProject(String),
    #[error("unknown branch {0}")]
    UnknownBranch(String),
    #[error("event {0} is not a reversible human decision")]
    NotReversible(String),
    #[error("import bundle error: {0}")]
    Bundle(String),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, DeskError>;
