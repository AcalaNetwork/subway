use async_trait::async_trait;
use serde::Deserialize;

use crate::{extension::Extension, middleware::ExtensionRegistry};

pub struct MergeSubscription {
    pub config: MergeSubscriptionConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MergeSubscriptionConfig {
    #[serde(default)]
    pub keep_alive_seconds: Option<u64>,
}

#[async_trait]
impl Extension for MergeSubscription {
    type Config = MergeSubscriptionConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl MergeSubscription {
    pub fn new(config: MergeSubscriptionConfig) -> Self {
        Self { config }
    }
}
