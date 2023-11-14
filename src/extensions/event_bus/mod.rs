use async_trait::async_trait;
use serde::Deserialize;

use super::{Extension, ExtensionRegistry};

pub struct EventBus;

#[derive(Deserialize, Debug, Clone)]
pub struct EventBusConfig;

#[async_trait]
impl Extension for EventBus {
    type Config = EventBusConfig;

    async fn from_config(_config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(EventBus)
    }
}
