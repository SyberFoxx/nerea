//! Known OpenAI-compatible endpoints.
//!
//! Adding a provider is adding an entry here; the transport is shared.

pub struct ProviderPreset {
    pub id: &'static str,
    pub label: &'static str,
    /// Base URL *up to* the version segment; `/chat/completions` is appended.
    pub base_url: &'static str,
    /// Env var read before the keyring, useful for development.
    pub env_var: &'static str,
    pub default_model: &'static str,
    pub models: &'static [&'static str],
}

pub const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "opencode-go",
        label: "OpenCode Go",
        base_url: "https://opencode.ai/inference/go/openai/v1",
        env_var: "OPENCODE_API_KEY",
        default_model: "deepseek-v4.1-flash",
        // Pinned on purpose: OpenCode Go is only used for this model.
        models: &["deepseek-v4.1-flash"],
    },
    ProviderPreset {
        id: "deepseek",
        label: "DeepSeek",
        base_url: "https://api.deepseek.com",
        env_var: "DEEPSEEK_API_KEY",
        default_model: "deepseek-v4-flash",
        models: &["deepseek-v4-flash", "deepseek-v4-pro", "deepseek-flash"],
    },
];

impl ProviderPreset {
    /// Guards against silently billing a model the project does not use.
    pub fn allows_model(&self, model: &str) -> bool {
        self.models.contains(&model)
    }
}

pub fn all() -> &'static [ProviderPreset] {
    PRESETS
}

pub fn find(id: &str) -> Option<&'static ProviderPreset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_go_is_pinned_to_a_single_model() {
        let preset = find("opencode-go").expect("preset");
        assert_eq!(preset.models, &["deepseek-v4.1-flash"]);
        assert!(preset.allows_model("deepseek-v4.1-flash"));
        assert!(!preset.allows_model("deepseek-v4-pro"));
    }

    #[test]
    fn deepseek_accepts_only_its_own_models() {
        let preset = find("deepseek").expect("preset");
        assert!(preset.allows_model("deepseek-v4-flash"));
        assert!(!preset.allows_model("deepseek-v4.1-flash"));
    }
}
