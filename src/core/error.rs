use serde::Serialize;
use std::{fmt, io};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize)]
pub struct Error {
    pub code: String,
    pub message: String,
}

impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::new("IO_ERROR", e.to_string())
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new("VALIDATION_ERROR", e.to_string())
    }
}
