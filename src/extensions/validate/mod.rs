use async_trait::async_trait;
use serde::Deserialize;

use super::{Extension, ExtensionRegistry};

#[derive(Default)]
pub struct Validate {
    pub config: ValidateConfig,
}

#[derive(Deserialize, Default, Debug, Clone)]
pub struct ValidateConfig {
    pub ignore_methods: Vec<String>,
}

#[async_trait]
impl Extension for Validate {
    type Config = ValidateConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl Validate {
    pub fn new(config: ValidateConfig) -> Self {
        Self { config }
    }

    pub fn ignore(&self, method: &String) -> bool {
        self.config.ignore_methods.contains(method)
    }
}
