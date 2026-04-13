use thiserror::Error;

#[derive(Debug, Error)]
pub enum FagentError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("execution error: {0}")]
    Execution(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("prompt cancelled")]
    PromptCancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Figment(#[from] figment::Error),
    #[error(transparent)]
    Keyring(#[from] keyring::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

impl From<inquire::error::InquireError> for FagentError {
    fn from(value: inquire::error::InquireError) -> Self {
        match value {
            inquire::error::InquireError::OperationCanceled
            | inquire::error::InquireError::OperationInterrupted => Self::PromptCancelled,
            other => Self::Validation(format!("prompt failed: {other}")),
        }
    }
}

pub type Result<T> = std::result::Result<T, FagentError>;
