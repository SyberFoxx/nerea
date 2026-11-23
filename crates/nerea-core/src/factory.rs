use std::sync::Arc;

use thiserror::Error;

use crate::ai::presets::{self, ProviderPreset};
use crate::ai::fallback::FallbackProvider;
use crate::ai::openai_compat::OpenAiCompatProvider;
use crate::ai::AiProvider;
use crate::config::Config;
use crate::error::ProviderError;
use crate::secrets::{SecretError, SecretStore};

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("unknown provider `{0}` (known: {1})")]
    UnknownProvider(String, String),

    #[error("model `{model}` is not allowed for `{provider}` (allowed: {allowed})")]
    ModelNotAllowed {
        provider: String,
        model: String,
        allowed: String,
    },

    #[error("no API key for `{provider}`: run `nerea key set {provider}` or export {env_var}")]
    MissingKey {
        provider: String,
        env_var: &'static str,
    },

    #[error(transparent)]
    Secret(#[from] SecretError),

    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// Where API keys come from. Injectable so builds can be tested without a keyring.
pub trait KeySource {
    fn api_key(&self, preset: &ProviderPreset) -> Result<Option<String>, SecretError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Env,
    Keyring,
    Missing,
    Unavailable,
}

/// Environment first (development convenience), then the OS keyring.
pub struct PlatformKeys {
    store: SecretStore,
}

impl PlatformKeys {
    pub fn new(service: impl Into<String>) -> Self {
        Self { store: SecretStore::new(service) }
    }

    pub fn state(&self, preset: &ProviderPreset) -> KeyState {
        if env_key(preset).is_some() {
            return KeyState::Env;
        }
        match self.store.api_key(preset.id) {
            Ok(Some(_)) => KeyState::Keyring,
            Ok(None) => KeyState::Missing,
            Err(_) => KeyState::Unavailable,
        }
    }
}

fn env_key(preset: &ProviderPreset) -> Option<String> {
    std::env::var(preset.env_var)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

impl KeySource for PlatformKeys {
    fn api_key(&self, preset: &ProviderPreset) -> Result<Option<String>, SecretError> {
        if let Some(key) = env_key(preset) {
            return Ok(Some(key));
        }
        self.store.api_key(preset.id)
    }
}

pub fn find(provider_id: &str) -> Result<&'static ProviderPreset, BuildError> {
    presets::find(provider_id).ok_or_else(|| {
        let known: Vec<&str> = presets::all().iter().map(|preset| preset.id).collect();
        BuildError::UnknownProvider(provider_id.to_owned(), known.join(", "))
    })
}

pub fn build(
    provider_id: &str,
    model_override: Option<&str>,
    keys: &dyn KeySource,
) -> Result<Arc<dyn AiProvider>, BuildError> {
    let preset = find(provider_id)?;
    let model = model_override.unwrap_or(preset.default_model).to_owned();

    // Guards before any request is billed.
    if !preset.allows_model(&model) {
        return Err(BuildError::ModelNotAllowed {
            provider: preset.id.to_owned(),
            model,
            allowed: preset.models.join(", "),
        });
    }

    let api_key = keys.api_key(preset)?.ok_or_else(|| BuildError::MissingKey {
        provider: preset.id.to_owned(),
        env_var: preset.env_var,
    })?;

    let base_url = std::env::var("NEREA_BASE_URL").unwrap_or_else(|_| preset.base_url.to_owned());
    let mut transport = OpenAiCompatProvider::new(preset.id, base_url, api_key, model)?;

    if let Some(header) = preset.session_header {
        transport = transport.with_session_header(header)?;
    }

    Ok(Arc::new(transport))
}

pub struct Chain {
    pub provider: Arc<dyn AiProvider>,
    /// Non-fatal degradations, e.g. a fallback configured without a key.
    pub warnings: Vec<String>,
}

pub fn build_chain(
    config: &Config,
    keys: &dyn KeySource,
    allow_fallback: bool,
) -> Result<Chain, BuildError> {
    let primary = build(&config.provider, config.model.as_deref(), keys)?;
    let mut warnings = Vec::new();

    let provider: Arc<dyn AiProvider> = match config.fallback.as_ref().filter(|_| allow_fallback) {
        Some(fallback) => match build(&fallback.provider, fallback.model.as_deref(), keys) {
            Ok(secondary) => Arc::new(FallbackProvider::new(primary, secondary)),
            Err(err) => {
                warnings.push(format!(
                    "fallback `{}` unavailable, continuing without it: {err}",
                    fallback.provider
                ));
                primary
            }
        },
        None => primary,
    };

    Ok(Chain { provider, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeKeys(HashMap<&'static str, &'static str>);

    impl KeySource for FakeKeys {
        fn api_key(&self, preset: &ProviderPreset) -> Result<Option<String>, SecretError> {
            Ok(self.0.get(preset.id).map(|key| (*key).to_owned()))
        }
    }

    fn both_keys() -> FakeKeys {
        FakeKeys(HashMap::from([("opencode-go", "k1"), ("deepseek", "k2")]))
    }

    #[test]
    fn unknown_provider_lists_the_known_ones() {
        let err = build("nope", None, &both_keys()).err().expect("unknown provider");
        assert!(err.to_string().contains("opencode-go"), "{err}");
    }

    #[test]
    fn a_disallowed_model_is_rejected_before_any_request() {
        let err = build("opencode-go", Some("deepseek-v4-pro"), &both_keys())
            .err()
            .expect("a disallowed model must not build");
        match err {
            BuildError::ModelNotAllowed { allowed, .. } => {
                assert_eq!(allowed, "deepseek-v4.1-flash");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn a_missing_key_names_the_environment_variable() {
        let err = build("deepseek", None, &FakeKeys(HashMap::new())).err().expect("missing key");
        assert!(err.to_string().contains("DEEPSEEK_API_KEY"), "{err}");
    }

    #[test]
    fn a_fallback_without_a_key_warns_and_keeps_the_primary() {
        let keys = FakeKeys(HashMap::from([("opencode-go", "k1")]));
        let chain = build_chain(&Config::default(), &keys, true).expect("primary works");

        assert_eq!(chain.provider.id(), "opencode-go");
        assert_eq!(chain.warnings.len(), 1);
        assert!(chain.warnings[0].contains("deepseek"), "{:?}", chain.warnings);
    }

    #[test]
    fn a_fallback_with_a_key_builds_without_warnings() {
        let chain = build_chain(&Config::default(), &both_keys(), true).expect("both providers");
        assert!(chain.warnings.is_empty());
        assert_eq!(chain.provider.id(), "opencode-go");
        assert_eq!(chain.provider.model(), "deepseek-v4.1-flash");
    }

    #[test]
    fn disabling_the_fallback_skips_it_entirely() {
        let chain = build_chain(&Config::default(), &both_keys(), false).expect("primary only");
        assert!(chain.warnings.is_empty());
        assert_eq!(chain.provider.id(), "opencode-go");
    }
}
