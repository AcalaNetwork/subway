use std::{future::Future, net::SocketAddr};

use async_trait::async_trait;
use jsonrpsee::server::{
    middleware::proxy_get_request::ProxyGetRequestLayer,
    {RandomStringIdProvider, RpcModule, ServerBuilder, ServerHandle},
};
use serde::Deserialize;

use crate::{extension::Extension, middleware::ExtensionRegistry};

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
        _registry: &ExtensionRegistry,
    ) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl Server {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    pub async fn create_server<Fut: Future<Output = anyhow::Result<RpcModule<()>>>>(
        &self,
        builder: impl FnOnce() -> Fut,
    ) -> anyhow::Result<(SocketAddr, ServerHandle)> {
        let service_builder =
            tower::ServiceBuilder::new().option_layer(self.config.health.as_ref().map(|h| {
                ProxyGetRequestLayer::new(h.path.clone(), h.method.clone())
                    .expect("Invalid health config")
            }));

        let server = ServerBuilder::default()
            .set_middleware(service_builder)
            .max_connections(self.config.max_connections)
            .set_id_provider(RandomStringIdProvider::new(16))
            .build((self.config.listen_address.as_str(), self.config.port))
            .await?;

        let module = builder().await?;

        let addr = server.local_addr()?;
        let server = server.start(module)?;

        Ok((addr, server))
    }
}
