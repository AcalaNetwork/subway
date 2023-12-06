use async_trait::async_trait;
use http::header::HeaderValue;
use hyper::server::conn::AddrStream;
use hyper::service::Service;
use hyper::service::{make_service_fn, service_fn};
use jsonrpsee::server::{
    middleware::rpc::RpcServiceBuilder, stop_channel, RandomStringIdProvider, RpcModule, ServerBuilder, ServerHandle,
};
use jsonrpsee::Methods;
use serde::ser::StdError;
use serde::Deserialize;
use std::str::FromStr;
use std::sync::Arc;
use std::{future::Future, net::SocketAddr};
use tower::ServiceBuilder;
use tower_http::cors::{AllowOrigin, CorsLayer};

use super::{Extension, ExtensionRegistry};
use crate::extensions::rate_limit::{MethodWeights, RateLimitBuilder, XFF};

mod proxy_get_request;
use proxy_get_request::{ProxyGetRequestLayer, ProxyGetRequestMethod};

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
        rate_limit_builder: Option<Arc<RateLimitBuilder>>,
        rpc_method_weights: MethodWeights,
        rpc_module_builder: impl FnOnce() -> Fut,
    ) -> anyhow::Result<(SocketAddr, ServerHandle)> {
        let config = self.config.clone();

        let (stop_handle, server_handle) = stop_channel();
        let handle = stop_handle.clone();
        let rpc_module = rpc_module_builder().await?;

        // make_service handle each connection
        let make_service = make_service_fn(move |socket: &AddrStream| {
            let socket_ip = socket.remote_addr().ip().to_string();

            let http_middleware: ServiceBuilder<_> = tower::ServiceBuilder::new()
                .layer(cors_layer(config.cors.clone()).expect("Invalid CORS config"))
                .layer(
                    ProxyGetRequestLayer::new(
                        config
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

            let rpc_module = rpc_module.clone();
            let stop_handle = stop_handle.clone();
            let rate_limit_builder = rate_limit_builder.clone();
            let rpc_method_weights = rpc_method_weights.clone();

            async move {
                // service_fn handle each request
                Ok::<_, Box<dyn StdError + Send + Sync>>(service_fn(move |req| {
                    let mut socket_ip = socket_ip.clone();
                    let methods: Methods = rpc_module.clone().into();
                    let stop_handle = stop_handle.clone();
                    let http_middleware = http_middleware.clone();

                    if let Some(true) = rate_limit_builder.as_ref().map(|r| r.use_xff()) {
                        socket_ip = req.xxf_ip().unwrap_or(socket_ip);
                    }

                    let rpc_middleware = RpcServiceBuilder::new()
                        .option_layer(
                            rate_limit_builder
                                .as_ref()
                                .and_then(|r| r.ip_limit(socket_ip, rpc_method_weights.clone())),
                        )
                        .option_layer(
                            rate_limit_builder
                                .as_ref()
                                .and_then(|r| r.connection_limit(rpc_method_weights.clone())),
                        );

                    let service_builder = ServerBuilder::default()
                        .set_rpc_middleware(rpc_middleware)
                        .set_http_middleware(http_middleware)
                        .max_connections(config.max_connections)
                        .set_id_provider(RandomStringIdProvider::new(16))
                        .to_service_builder();

                    let mut service = service_builder.build(methods, stop_handle);
                    service.call(req)
                }))
            }
        });

        let ip_addr = std::net::IpAddr::from_str(&self.config.listen_address)?;
        let addr = SocketAddr::new(ip_addr, self.config.port);

        let server = hyper::Server::bind(&addr).serve(make_service);
        let addr = server.local_addr();

        tokio::spawn(async move {
            let graceful = server.with_graceful_shutdown(async move { handle.shutdown().await });
            graceful.await.unwrap()
        });

        Ok((addr, server_handle))
    }
}
