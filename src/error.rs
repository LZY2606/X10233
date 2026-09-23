use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("sqlite error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("input format error: {0}")]
    Format(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("integrity error: {0}")]
    Integrity(String),
}

impl AppError {
    pub fn format(msg: impl Into<String>) -> Self {
        AppError::Format(msg.into())
    }
    pub fn http_status(&self) -> u16 {
        match self {
            AppError::Format(_) => 400,
            AppError::Conflict(_) => 409,
            AppError::NotFound(_) => 404,
            AppError::Integrity(_) => 422,
            _ => 500,
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
