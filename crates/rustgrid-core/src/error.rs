use thiserror::Error;

/// The message is the raw driver text; the UI layer supplies the localized category label, so no
/// hard-coded English prefix leaks into the interface.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    DriverNotFound(String),

    #[error("{0}")]
    Connection(String),

    #[error("{0}")]
    Authentication(String),

    #[error("{0}")]
    Query(String),

    #[error("{0}")]
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
