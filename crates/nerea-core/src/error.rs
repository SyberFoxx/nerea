use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    /// Normalized from each provider's error envelope. Callers branch on `status`.
    #[error("provider rejected the request ({status}): {message}")]
    Api {
        status: u16,
        code: Option<String>,
        message: String,
    },

    #[error("provider returned an invalid payload: {0}")]
    Decode(#[from] serde_json::Error),

    #[error("protocol violation: {0}")]
    Protocol(String),
}
