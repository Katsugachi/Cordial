//! Error types shared by the harness.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("could not find a data directory for this user")]
    NoDataDir,
    #[error("model is not ready: {0}")]
    ModelNotReady(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[error(
        "the prompt does not fit the context window ({used} tokens needed, {available} available)"
    )]
    ContextOverflow { used: usize, available: usize },
    #[error("tool `{tool}` failed: {message}")]
    Tool { tool: String, message: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("no browser was found. Install Chrome, Chromium or Edge, or use links instead.")]
    NoBrowser,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not parse JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

impl From<cordial_llama::LlamaError> for CoreError {
    fn from(err: cordial_llama::LlamaError) -> Self {
        CoreError::Inference(err.to_string())
    }
}
