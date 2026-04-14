use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("dropped: sender or receiver was released")]
    Dropped,
    #[error("unsupported codec or config on this platform")]
    Unsupported,
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("platform error: {0}")]
    Platform(String),
    #[error("no backend available for this platform")]
    NoBackend,
}
