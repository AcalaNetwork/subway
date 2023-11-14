use async_trait::async_trait;
use serde::Deserialize;

use super::{Extension, ExtensionRegistry};

pub struct Cache {
    pub config: CacheConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CacheConfig {
    // None means no cache expiration
    #[serde(default)]
    pub default_ttl_seconds: Option<u64>,
    pub default_size: usize,
}

#[async_trait]
impl Extension for Cache {
    type Config = CacheConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl Cache {
    pub fn new(config: CacheConfig) -> Self {
        Self { config }
    }
}
