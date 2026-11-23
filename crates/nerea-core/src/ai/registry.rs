use std::{collections::HashMap, sync::Arc};

use super::AiProvider;

/// Holds heterogeneous providers behind `dyn AiProvider`.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn AiProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adding a provider is one line at the composition root.
    pub fn register(&mut self, provider: Arc<dyn AiProvider>) {
        self.providers.insert(provider.id().to_owned(), provider);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn AiProvider>> {
        self.providers.get(id).cloned()
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }
}
