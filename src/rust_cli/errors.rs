use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Mongo(#[from] mongodb::error::Error),
    #[error("{0}")]
    Bson(#[from] mongodb::bson::error::Error),
    #[error("{0}")]
    Url(#[from] url::ParseError),
    #[error("{0}")]
    Walkdir(#[from] walkdir::Error),
    #[error("invalid EVM address: {0}")]
    InvalidAddress(String),
    #[error("run_id does not exist: {0}")]
    RunNotFound(String),
}

pub type AppResult<T> = Result<T, AppError>;

pub fn msg(text: impl Into<String>) -> AppError {
    AppError::Message(text.into())
}
