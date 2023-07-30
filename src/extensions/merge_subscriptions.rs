use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    extension::Extension,
    helpers::{self, errors},
    utils::TypeRegistryRef,
};

pub struct MergeSubscriptions {
    config: MergeSubscriptionsConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MergeSubscriptionsConfig {
    #[serde(default)]
    pub keep_alive_seconds: Option<u64>,
}

#[async_trait]
impl Extension for MergeSubscriptions {
    type Config = MergeSubscriptionsConfig;

    async fn from_config(
        config: &Self::Config,
        _registry: &TypeRegistryRef,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl MergeSubscriptions {
    pub fn new(config: MergeSubscriptionsConfig) -> Self {
        Self { config }
    }
}
