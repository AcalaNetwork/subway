use async_trait::async_trait;
use futures::FutureExt;
use http::header::HeaderValue;
use jsonrpsee::server::{
    middleware::rpc::RpcServiceBuilder, serve_with_graceful_shutdown, stop_channel, ws, BatchRequestConfig,
    RandomStringIdProvider, RpcModule, ServerHandle, StopHandle, TowerServiceBuilder,
};
use jsonrpsee::Methods;
use tokio::net::TcpListener;

use serde::Deserialize;

use std::str::FromStr;
use std::sync::Arc;
use std::{future::Future, net::SocketAddr};
use tower::layer::layer_fn;
use tower::Service;
use tower_http::cors::{AllowOrigin, CorsLayer};

use super::{Extension, ExtensionRegistry};
use crate::extensions::rate_limit::{MethodWeights, RateLimitBuilder, XFF};
pub use prometheus::Protocol;

mod prometheus;
mod proxy_get_request;

use crate::extensions::prometheus::RpcMetrics;
use crate::extensions::server::prometheus::PrometheusService;
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
    #[serde(default = "default_max_subscriptions_per_connection")]
    pub max_subscriptions_per_connection: u32,
    pub max_batch_size: Option<u32>,
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

fn default_max_subscriptions_per_connection() -> u32 {
    1024
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
        rpc_metrics: RpcMetrics,
        rpc_module_builder: impl FnOnce() -> Fut,
    ) -> anyhow::Result<(SocketAddr, ServerHandle)> {
        let config = self.config.clone();

        let rpc_module = rpc_module_builder().await?;

        let http_middleware = tower::ServiceBuilder::new()
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

        let batch_request_config = match config.max_batch_size {
            Some(0) => BatchRequestConfig::Disabled,
            Some(max_size) => BatchRequestConfig::Limit(max_size),
            None => BatchRequestConfig::Unlimited,
        };

        let ip_addr = std::net::IpAddr::from_str(&self.config.listen_address)?;
        let addr = SocketAddr::new(ip_addr, self.config.port);

        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;

        // This state is cloned for every connection
        // all these types based on Arcs and it should
        // be relatively cheap to clone them.
        //
        // Make sure that nothing expensive is cloned here
        // when doing this or use an `Arc`.
        #[derive(Clone)]
        struct PerConnection<RpcMiddleware, HttpMiddleware> {
            methods: Methods,
            stop_handle: StopHandle,
            rpc_metrics: RpcMetrics,
            svc_builder: TowerServiceBuilder<RpcMiddleware, HttpMiddleware>,
            rate_limit_builder: Option<Arc<RateLimitBuilder>>,
            rpc_method_weights: MethodWeights,
        }

        // Each RPC call/connection get its own `stop_handle`
        // to able to determine whether the server has been stopped or not.
        //
        // To keep the server running the `server_handle`
        // must be kept and it can also be used to stop the server.
        let (stop_handle, server_handle) = stop_channel();

        let per_conn = PerConnection {
            methods: rpc_module.into(),
            stop_handle: stop_handle.clone(),
            rpc_metrics,
            svc_builder: jsonrpsee::server::Server::builder()
                .set_http_middleware(http_middleware)
                .set_batch_request_config(batch_request_config)
                .max_connections(config.max_connections)
                .max_subscriptions_per_connection(config.max_subscriptions_per_connection)
                .set_id_provider(RandomStringIdProvider::new(16))
                .to_service_builder(),
            rate_limit_builder,
            rpc_method_weights,
        };

        tokio::spawn(async move {
            loop {
                // The `tokio::select!` macro is used to wait for either of the
                // listeners to accept a new connection or for the server to be
                // stopped.
                let (sock, remote_addr) = tokio::select! {
                    res = listener.accept() => {
                        match res {
                            Ok((stream, remote_addr)) => (stream, remote_addr),
                            Err(e) => {
                            tracing::error!("failed to accept v4 connection: {:?}", e);
                                continue;
                            }
                        }
                    }
                    _ = per_conn.stop_handle.clone().shutdown() => break,
                };

                let per_conn2 = per_conn.clone();

                // service_fn handle each connection
                let svc = tower::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let PerConnection {
                        methods,
                        stop_handle,
                        rpc_metrics,
                        svc_builder,
                        rate_limit_builder,
                        rpc_method_weights,
                    } = per_conn2.clone();

                    let is_websocket = ws::is_upgrade_request(&req);
                    let protocol = if is_websocket { Protocol::Ws } else { Protocol::Http };

                    let mut socket_ip = remote_addr.ip().to_string();
                    if let Some(true) = rate_limit_builder.as_ref().map(|r| r.use_xff()) {
                        socket_ip = req.xxf_ip().unwrap_or(socket_ip);
                    }

                    let call_metrics = rpc_metrics.call_metrics();

                    async move {
                        let rpc_middleware =
                            RpcServiceBuilder::new()
                                .option_layer(
                                    rate_limit_builder
                                        .as_ref()
                                        .and_then(|r| r.ip_limit(socket_ip, rpc_method_weights.clone())),
                                )
                                .option_layer(
                                    rate_limit_builder
                                        .as_ref()
                                        .and_then(|r| r.connection_limit(rpc_method_weights.clone())),
                                )
                                .option_layer(call_metrics.as_ref().map(move |(a, b, c)| {
                                    layer_fn(move |s| PrometheusService::new(s, protocol, a, b, c))
                                }));

                        let mut service = svc_builder
                            .set_rpc_middleware(rpc_middleware)
                            .build(methods, stop_handle);

                        if is_websocket {
                            let on_ws_close = service.on_session_closed();
                            rpc_metrics.ws_open();
                            tokio::spawn(async move {
                                on_ws_close.await;
                                rpc_metrics.ws_closed();
                            });
                        }

                        service.call(req).await.map_err(|e| anyhow::anyhow!("{:?}", e))
                    }
                    .boxed()
                });

                tokio::spawn(serve_with_graceful_shutdown(sock, svc, stop_handle.clone().shutdown()));
            }
        });

        Ok((addr, server_handle))
    }
}
