use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    extension::Extension,
    helpers::{self, errors},
    utils::TypeRegistryRef,
};

pub struct Cache {
    config: CacheConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CacheConfig {
    // None means no cache expiration
    #[serde(default)]
    pub default_ttl_seconds: Option<u64>,
    pub default_size: u32,
}

#[async_trait]
impl Extension for Cache {
    type Config = CacheConfig;

    async fn from_config(
        config: &Self::Config,
        _registry: &TypeRegistryRef,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl Cache {
    pub fn new(config: CacheConfig) -> Self {
        Self { config }
    }
}
