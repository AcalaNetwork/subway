use async_trait::async_trait;
use serde::Deserialize;

use crate::{extension::Extension, utils::TypeRegistryRef};

pub struct EventBus {
    config: EventBusConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct EventBusConfig {
    // None means no EventBus expiration
    #[serde(default)]
    pub default_ttl_seconds: Option<u64>,
    pub default_size: u32,
}

#[async_trait]
impl Extension for EventBus {
    type Config = EventBusConfig;

    async fn from_config(
        config: &Self::Config,
        _registry: &TypeRegistryRef,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl EventBus {
    pub fn new(config: EventBusConfig) -> Self {
        Self { config }
    }
}
