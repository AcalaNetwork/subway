use std::{future::Future, net::SocketAddr};

use async_trait::async_trait;
use http::header::HeaderValue;
use jsonrpsee::server::{
    middleware::rpc::RpcServiceBuilder, RandomStringIdProvider, RpcModule, ServerBuilder, ServerHandle,
};
use serde::Deserialize;
use tower_http::cors::{AllowOrigin, CorsLayer};

use super::{Extension, ExtensionRegistry};
use proxy_get_request::ProxyGetRequestLayer;

use self::proxy_get_request::ProxyGetRequestMethod;

mod proxy_get_request;
mod rate_limit;

pub struct SubwayServerBuilder {
    pub config: ServerConfig,
}

#[derive(Deserialize, Debug, Clone)]
pub struct HttpMethodsConfig {
    pub path: String,
    pub method: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum ItemOrList<T> {
    Item(T),
    List(Vec<T>),
}

impl<T> ItemOrList<T> {
    fn into_list(self) -> Vec<T> {
        match self {
            ItemOrList::Item(item) => vec![item],
            ItemOrList::List(list) => list,
        }
    }
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
    #[serde(default)]
    pub cors: Option<ItemOrList<String>>,
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
}

fn default_request_timeout_seconds() -> u64 {
    120
}

#[derive(Deserialize, Default, Debug, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    #[default]
    Second,
    Minute,
    Hour,
}

#[derive(Deserialize, Debug, Copy, Clone, Default)]
pub struct RateLimitConfig {
    // 0 means no limit
    pub burst: u32,
    pub period: Period,
    #[serde(default = "default_jitter_millis")]
    pub jitter_millis: u64,
}

fn default_jitter_millis() -> u64 {
    1000
}

#[async_trait]
impl Extension for SubwayServerBuilder {
    type Config = ServerConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

fn cors_layer(cors: Option<ItemOrList<String>>) -> anyhow::Result<CorsLayer> {
    let origins = cors.map(|c| c.into_list()).unwrap_or_default();

    match origins.as_slice() {
        [] => Ok(CorsLayer::new()),
        [origin] if origin == "*" || origin == "all" => Ok(CorsLayer::permissive()),
        origins => {
            let list = origins
                .iter()
                .map(|o| HeaderValue::from_str(o))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CorsLayer::new().allow_origin(AllowOrigin::list(list)))
        }
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
        let rate_limit_config = self.config.rate_limit;

        let rpc_middleware = RpcServiceBuilder::new()
            // rate limit per connection
            .layer_fn(move |service| rate_limit::RateLimit::new(service, rate_limit_config));

        let service_builder = tower::ServiceBuilder::new()
            .layer(cors_layer(self.config.cors.clone()).expect("Invalid CORS config"))
            .layer(
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
            .set_rpc_middleware(rpc_middleware)
            .set_http_middleware(service_builder)
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
