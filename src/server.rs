use futures::FutureExt;
use jsonrpsee::core::JsonValue;
use jsonrpsee::server::{RandomStringIdProvider, RpcModule, ServerBuilder, ServerHandle};
use jsonrpsee::types::ErrorObjectOwned;
use serde_json::json;
use std::time::Duration;
use std::{net::SocketAddr, num::NonZeroUsize, sync::Arc};

use crate::cache::new_cache;
use crate::helper::errors;
use crate::{
    api::{EthApi, SubstrateApi},
    client::Client,
    config::Config,
    middleware::{
        block_tag::BlockTagMiddleware,
        cache::CacheMiddleware,
        call::{self, CallRequest},
        inject_params::{inject, InjectParamsMiddleware},
        merge_subscription::MergeSubscriptionMiddleware,
        subscription::{self, SubscriptionRequest},
        Middleware, Middlewares,
    },
};

// TODO: https://github.com/paritytech/jsonrpsee/issues/985
fn string_to_static_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

pub async fn start_server(
    config: &Config,
    client: Client,
) -> anyhow::Result<(SocketAddr, ServerHandle)> {
    let service_builder = tower::ServiceBuilder::new();

    let server = ServerBuilder::default()
        .set_middleware(service_builder)
        .max_connections(config.server.max_connections)
        .set_id_provider(RandomStringIdProvider::new(16))
        .build((config.server.listen_address.as_str(), config.server.port))
        .await?;

    let mut module = RpcModule::new(());

    let client = Arc::new(client);

    let mut sub_api: Option<Arc<SubstrateApi>> = None;
    let mut eth_api: Option<Arc<EthApi>> = None;

    let mut get_sub_api = || {
        if sub_api.is_none() {
            sub_api = Some(Arc::new(SubstrateApi::new(
                client.clone(),
                Duration::from_secs(config.stale_timeout_seconds),
            )));
        }
        sub_api.as_ref().unwrap().clone()
    };

    let mut get_eth_api = || {
        if eth_api.is_none() {
            eth_api = Some(Arc::new(EthApi::new(
                client.clone(),
                Duration::from_secs(config.stale_timeout_seconds),
            )));
        }
        eth_api.as_ref().unwrap().clone()
    };

    let upstream = Arc::new(call::UpstreamMiddleware::new(client.clone()));

    for method in &config.rpcs.methods {
        let mut list: Vec<Arc<dyn Middleware<_, _>>> = vec![];

        if let Some(index) = method
            .params
            .iter()
            .position(|p| p.ty == "BlockTag" && p.inject)
        {
            list.push(Arc::new(BlockTagMiddleware::new(get_eth_api(), index)))
        }

        if let Some(inject_type) = inject(&method.params) {
            list.push(Arc::new(InjectParamsMiddleware::new(
                get_sub_api(),
                inject_type,
                method.params.clone(),
            )));
        }

        if let Some(cache_size) = NonZeroUsize::new(method.cache) {
            // each method has it's own cache
            let cache = new_cache(
                cache_size,
                config.cache_ttl_seconds.map(Duration::from_secs),
            );
            list.push(Arc::new(CacheMiddleware::new(cache)));
        }
        list.push(upstream.clone());

        let middlewares = Arc::new(Middlewares::new(
            list,
            Arc::new(|_| async { Err(errors::failed("Bad configuration")) }.boxed()),
        ));

        let method_name = string_to_static_str(method.method.clone());
        module.register_async_method(method_name, move |params, _| {
            let middlewares = middlewares.clone();
            async move {
                let parsed = params.parse::<JsonValue>()?;
                let params = if parsed == JsonValue::Null {
                    vec![]
                } else {
                    parsed
                        .as_array()
                        .ok_or_else(|| errors::invalid_params(""))?
                        .to_owned()
                };
                middlewares
                    .call(CallRequest::new(method_name, params))
                    .await
            }
        })?;
    }

    let upstream = Arc::new(subscription::UpstreamMiddleware::new(client.clone()));

    for subscription in &config.rpcs.subscriptions {
        let subscribe_name = string_to_static_str(subscription.subscribe.clone());
        let unsubscribe_name = string_to_static_str(subscription.unsubscribe.clone());
        let name = string_to_static_str(subscription.name.clone());

        let mut list: Vec<Arc<dyn Middleware<_, _>>> = vec![];
        match subscription.merge_strategy {
            Some(merge_strategy) => list.push(Arc::new(MergeSubscriptionMiddleware::new(
                client.clone(),
                merge_strategy,
                config.merge_subscription_keep_alive_seconds,
            ))),
            None => list.push(upstream.clone()),
        };

        let middlewares = Arc::new(Middlewares::new(
            list,
            Arc::new(|_| async { Err("Bad configuration".into()) }.boxed()),
        ));

        module.register_subscription(
            subscribe_name,
            name,
            unsubscribe_name,
            move |params, sink, _| {
                let middlewares = middlewares.clone();
                async move {
                    let parsed = params.parse::<JsonValue>()?;
                    let params = if parsed == JsonValue::Null {
                        vec![]
                    } else {
                        parsed
                            .as_array()
                            .ok_or_else(|| errors::invalid_params(""))?
                            .to_owned()
                    };
                    middlewares
                        .call(SubscriptionRequest {
                            subscribe: subscribe_name.into(),
                            params,
                            unsubscribe: unsubscribe_name.into(),
                            sink,
                        })
                        .await
                }
            },
        )?;
    }

    for (alias_old, alias_new) in &config.rpcs.aliases {
        let alias_old = string_to_static_str(alias_old.clone());
        let alias_new = string_to_static_str(alias_new.clone());
        module.register_alias(alias_new, alias_old)?;
    }

    let mut rpc_methods = module
        .method_names()
        .map(|x| x.to_owned())
        .collect::<Vec<_>>();

    rpc_methods.sort();

    module.register_method("rpc_methods", move |_, _| {
        Ok::<JsonValue, ErrorObjectOwned>(json!({
            "version": 1,
            "methods": rpc_methods
        }))
    })?;

    let addr = server.local_addr()?;
    let server = server.start(module)?;

    Ok((addr, server))
}

#[cfg(test)]
mod tests {
    use jsonrpsee::{
        core::client::ClientT,
        rpc_params,
        server::ServerHandle,
        ws_client::{WsClient, WsClientBuilder},
        RpcModule,
    };

    use super::*;
    use crate::config::{RpcDefinitions, RpcMethod, ServerConfig};

    const PHO: &str = "pho";
    const BAR: &str = "bar";
    const WS_SERVER_ENDPOINT: &str = "127.0.0.1:9955";

    async fn server() -> (String, ServerHandle) {
        let config = Config {
            endpoints: vec![format!("ws://{}", WS_SERVER_ENDPOINT)],
            stale_timeout_seconds: 60,
            cache_ttl_seconds: None,
            merge_subscription_keep_alive_seconds: None,
            server: ServerConfig {
                listen_address: "127.0.0.1".to_string(),
                port: 9944,
                max_connections: 1024,
            },
            rpcs: RpcDefinitions {
                methods: vec![RpcMethod {
                    method: PHO.to_string(),
                    params: vec![],
                    cache: 0,
                }],
                subscriptions: vec![],
                aliases: vec![],
            },
            telemetry: None,
        };
        let client = Client::new(&config.endpoints).await.unwrap();
        let (addr, server) = start_server(&config, client).await.unwrap();
        (format!("ws://{}", addr), server)
    }

    async fn ws_server(url: &str) -> (String, ServerHandle) {
        let server = ServerBuilder::default()
            .max_request_body_size(u32::MAX)
            .max_response_body_size(u32::MAX)
            .max_connections(10 * 1024)
            .build(url)
            .await
            .unwrap();

        let mut module = RpcModule::new(());
        module
            .register_method(PHO, |_, _| Ok(BAR.to_string()))
            .unwrap();
        let addr = format!("ws://{}", server.local_addr().unwrap());
        let handle = server.start(module).unwrap();
        (addr, handle)
    }

    async fn ws_client(url: &str) -> WsClient {
        WsClientBuilder::default()
            .max_request_size(u32::MAX)
            .max_concurrent_requests(1024 * 1024)
            .build(url)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn null_param_works() {
        let (_url1, _server1) = ws_server(WS_SERVER_ENDPOINT).await;
        let (url, _server) = server().await;
        let client = ws_client(&url).await;
        assert_eq!(
            BAR,
            client
                .request::<String, _>(PHO, rpc_params!())
                .await
                .unwrap()
        );
    }
}
