use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    extension::Extension,
    helpers::{self, errors},
    utils::TypeRegistryRef,
};

pub struct Server {
    config: ServerConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct HealthConfig {
    pub path: String,
    pub method: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub listen_address: String,
    pub max_connections: u32,
    #[serde(default)]
    pub health: Option<HealthConfig>,
}

#[async_trait]
impl Extension for Server {
    type Config = ServerConfig;

    async fn from_config(
        config: &Self::Config,
        _registry: &TypeRegistryRef,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl Server {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }
}
