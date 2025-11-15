use std::fmt;

#[derive(Debug)]
pub enum Error {
    ChronoError(String),
    SqlxError(sqlx::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ChronoError(msg) => write!(f, "Chrono error: {}", msg),
            Error::SqlxError(err) => write!(f, "SQLx error: {}", err),
        }
    }
}

impl std::error::Error for Error {}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Error::SqlxError(err)
    }
}
