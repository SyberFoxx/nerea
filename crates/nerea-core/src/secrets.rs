use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("system keyring unavailable: {0}")]
    Keyring(#[from] keyring::Error),
}

/// API keys live in the OS keyring, never on disk in plain text.
///
/// On Linux this resolves to Secret Service over D-Bus (pure Rust, no libdbus).
/// It requires a running keyring daemon (gnome-keyring, kwallet).
pub struct SecretStore {
    service: String,
}

impl SecretStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self { service: service.into() }
    }

    pub fn set_api_key(&self, provider_id: &str, key: &str) -> Result<(), SecretError> {
        keyring::Entry::new(&self.service, provider_id)?.set_password(key)?;
        Ok(())
    }

    pub fn api_key(&self, provider_id: &str) -> Result<Option<String>, SecretError> {
        match keyring::Entry::new(&self.service, provider_id)?.get_password() {
            Ok(key) => Ok(Some(key)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    pub fn delete_api_key(&self, provider_id: &str) -> Result<(), SecretError> {
        match keyring::Entry::new(&self.service, provider_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}
