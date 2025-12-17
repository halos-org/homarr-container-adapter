//! Error types for the adapter

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AdapterError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Homarr API error: {0}")]
    HomarrApi(String),

    #[error("State file error: {0}")]
    State(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("Tracing error: {0}")]
    Tracing(#[from] tracing::subscriber::SetGlobalDefaultError),
}

pub type Result<T> = std::result::Result<T, AdapterError>;
