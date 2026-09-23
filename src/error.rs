use std::fmt;

#[derive(Debug)]
pub enum DeskError {
    Sql(String),
    Io(String),
    Import {
        fatal: Vec<Diagnostic>,
        warnings: Vec<Diagnostic>,
    },
    BadRequest(String),
    NotFound(String),
    Conflict(String),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub line: Option<i64>,
    pub field: Option<String>,
    pub message: String,
    pub raw: Option<String>,
}

impl Diagnostic {
    pub fn fatal(code: impl Into<String>, line: Option<i64>, message: impl Into<String>) -> Self {
        Diagnostic { code: code.into(), line, field: None, message: message.into(), raw: None }
    }
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }
    pub fn with_raw(mut self, raw: impl Into<String>) -> Self {
        self.raw = Some(raw.into());
        self
    }
}

pub type Result<T> = std::result::Result<T, DeskError>;

impl fmt::Display for DeskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeskError::Sql(s) => write!(f, "sqlite: {s}"),
            DeskError::Io(s) => write!(f, "io: {s}"),
            DeskError::Import { fatal, warnings } => write!(
                f,
                "import failed with {} fatal error(s) and {} warning(s)",
                fatal.len(),
                warnings.len()
            ),
            DeskError::BadRequest(s) => write!(f, "{s}"),
            DeskError::NotFound(s) => write!(f, "not found: {s}"),
            DeskError::Conflict(s) => write!(f, "conflict: {s}"),
        }
    }
}
impl std::error::Error for DeskError {}

impl From<rusqlite::Error> for DeskError {
    fn from(e: rusqlite::Error) -> Self {
        DeskError::Sql(e.to_string())
    }
}
impl From<std::io::Error> for DeskError {
    fn from(e: std::io::Error) -> Self {
        DeskError::Io(e.to_string())
    }
}
