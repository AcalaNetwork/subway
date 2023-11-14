use std::{future::Future, net::SocketAddr};

use async_trait::async_trait;
use jsonrpsee::server::{RandomStringIdProvider, RpcModule, ServerBuilder, ServerHandle};
use serde::Deserialize;

use super::{Extension, ExtensionRegistry};
use proxy_get_request::ProxyGetRequestLayer;

use self::proxy_get_request::ProxyGetRequestMethod;

mod proxy_get_request;

pub struct SubwayServerBuilder {
    pub config: ServerConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct HttpMethodsConfig {
    pub path: String,
    pub method: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub listen_address: String,
    pub max_connections: u32,
    #[serde(default)]
    pub http_methods: Vec<HttpMethodsConfig>,
    #[serde(default = "default_request_timeout_seconds")]
    pub request_timeout_seconds: u64,
}

fn default_request_timeout_seconds() -> u64 {
    120
}

#[async_trait]
impl Extension for SubwayServerBuilder {
    type Config = ServerConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl SubwayServerBuilder {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    pub async fn build<Fut: Future<Output = anyhow::Result<RpcModule<()>>>>(
        &self,
        builder: impl FnOnce() -> Fut,
    ) -> anyhow::Result<(SocketAddr, ServerHandle)> {
        let service_builder = tower::ServiceBuilder::new().layer(
            ProxyGetRequestLayer::new(
                self.config
                    .http_methods
                    .iter()
                    .map(|m| ProxyGetRequestMethod {
                        path: m.path.clone(),
                        method: m.method.clone(),
                    })
                    .collect(),
            )
            .expect("Invalid health config"),
        );

        let server = ServerBuilder::default()
            .set_middleware(service_builder)
            .max_connections(self.config.max_connections)
            .set_id_provider(RandomStringIdProvider::new(16))
            .build((self.config.listen_address.as_str(), self.config.port))
            .await?;

        let module = builder().await?;

        let addr = server.local_addr()?;
        let server = server.start(module);

        Ok((addr, server))
    }
}
