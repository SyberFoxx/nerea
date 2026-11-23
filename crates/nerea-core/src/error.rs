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

impl ProviderError {
    /// Whether another provider is worth trying for this failure.
    ///
    /// Credentials are per provider, so auth rejections qualify too. A malformed
    /// payload does not: that is our adapter's bug, not a provider outage.
    pub fn is_failover_worthy(&self) -> bool {
        match self {
            Self::Network(_) => true,
            Self::Api { status, .. } => matches!(status, 401 | 403 | 408 | 429 | 500..=599),
            Self::Decode(_) | Self::Protocol(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(status: u16) -> ProviderError {
        ProviderError::Api { status, code: None, message: "boom".into() }
    }

    #[test]
    fn transient_and_auth_statuses_are_failover_worthy() {
        for status in [401, 403, 408, 429, 500, 502, 503, 599] {
            assert!(api(status).is_failover_worthy(), "status {status}");
        }
    }

    #[test]
    fn client_errors_and_broken_payloads_are_not() {
        for status in [400, 404, 409, 422] {
            assert!(!api(status).is_failover_worthy(), "status {status}");
        }
        assert!(!ProviderError::Protocol("no choices".into()).is_failover_worthy());
        let decode = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        assert!(!ProviderError::Decode(decode).is_failover_worthy());
    }
}
