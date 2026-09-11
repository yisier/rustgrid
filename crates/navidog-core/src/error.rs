use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("driver not found: {0}")]
    DriverNotFound(String),

    #[error("connection failed: {0}")]
    Connection(String),

    #[error("query failed: {0}")]
    Query(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("{0}")]
    Other(Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl Error {
    pub fn other(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Other(Box::new(error))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
