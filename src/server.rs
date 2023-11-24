use std::{net::SocketAddr, sync::Arc};

use futures::FutureExt;
use jsonrpsee::{
    core::JsonValue,
    server::{RpcModule, ServerHandle},
    types::ErrorObjectOwned,
};
use opentelemetry::trace::FutureExt as _;
use serde_json::json;

use crate::utils::TypeRegistryRef;
use crate::{
    config::Config,
    extensions::server::SubwayServerBuilder,
    middlewares::{factory, CallRequest, Middlewares, SubscriptionRequest},
    utils::{errors, telemetry},
};

// TODO: https://github.com/paritytech/jsonrpsee/issues/985
fn string_to_static_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

pub struct SubwayServerHandle {
    pub handle: ServerHandle,
    pub addr: SocketAddr,
    pub extensions: TypeRegistryRef,
}

pub async fn build(config: Config) -> anyhow::Result<SubwayServerHandle> {
    // create extensions registry from config
    let extensions_registry = config
        .extensions
        .create_registry()
        .await
        .expect("Failed to create extensions registry");

    // get the server extension
    let server_builder = extensions_registry
        .read()
        .await
        .get::<SubwayServerBuilder>()
        .expect("Server extension not found");

    let request_timeout_seconds = server_builder.config.request_timeout_seconds;

    let registry = extensions_registry.clone();
    let (addr, handle) = server_builder
        .build(move || async move {
            let mut module = RpcModule::new(());

            let tracer = telemetry::Tracer::new("server");

            // register methods from config
            for method in config.rpcs.methods {
                let mut method_middlewares: Vec<Arc<_>> = vec![];

                for middleware_name in &config.middlewares.methods {
                    if let Some(middleware) =
                        factory::create_method_middleware(middleware_name, &method, &registry).await
                    {
                        method_middlewares.push(middleware.into());
                    }
                }

                let method_middlewares = Middlewares::new(
                    method_middlewares,
                    Arc::new(|_, _| async { Err(errors::failed("Bad configuration")) }.boxed()),
                );

                let method_name = string_to_static_str(method.method.clone());

                module.register_async_method(method_name, move |params, _| {
                    let method_middlewares = method_middlewares.clone();
                    async move {
                        let parsed = params.parse::<JsonValue>()?;
                        let params = if parsed == JsonValue::Null {
                            vec![]
                        } else {
                            parsed.as_array().ok_or_else(|| errors::invalid_params(""))?.to_owned()
                        };

                        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                        let timeout = tokio::time::Duration::from_secs(request_timeout_seconds);

                        method_middlewares
                            .call(CallRequest::new(method_name, params), result_tx, timeout)
                            .await;

                        let result = result_rx
                            .await
                            .map_err(|_| errors::map_error(jsonrpsee::core::Error::RequestTimeout))?;

                        match result.as_ref() {
                            Ok(_) => tracer.span_ok(),
                            Err(err) => {
                                tracer.span_error(format!("{}", err));
                            }
                        };

                        result
                    }
                    .with_context(tracer.context(method_name))
                })?;
            }

            // register subscriptions from config
            for subscription in config.rpcs.subscriptions {
                let subscribe_name = string_to_static_str(subscription.subscribe.clone());
                let unsubscribe_name = string_to_static_str(subscription.unsubscribe.clone());
                let name = string_to_static_str(subscription.name.clone());

                let mut subscription_middlewares: Vec<Arc<_>> = vec![];

                for middleware_name in &config.middlewares.subscriptions {
                    if let Some(middleware) =
                        factory::create_subscription_middleware(middleware_name, &subscription, &registry).await
                    {
                        subscription_middlewares.push(middleware.into());
                    }
                }

                let subscription_middlewares = Middlewares::new(
                    subscription_middlewares,
                    Arc::new(|_, _| async { Err("Bad configuration".into()) }.boxed()),
                );

                module.register_subscription(
                    subscribe_name,
                    name,
                    unsubscribe_name,
                    move |params, pending_sink, _| {
                        let subscription_middlewares = subscription_middlewares.clone();
                        async move {
                            let parsed = params.parse::<JsonValue>()?;
                            let params = if parsed == JsonValue::Null {
                                vec![]
                            } else {
                                parsed.as_array().ok_or_else(|| errors::invalid_params(""))?.to_owned()
                            };

                            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                            let timeout = tokio::time::Duration::from_secs(request_timeout_seconds);

                            subscription_middlewares
                                .call(
                                    SubscriptionRequest {
                                        subscribe: subscribe_name.into(),
                                        params,
                                        unsubscribe: unsubscribe_name.into(),
                                        pending_sink,
                                    },
                                    result_tx,
                                    timeout,
                                )
                                .await;

                            let result = result_rx
                                .await
                                .map_err(|_| errors::map_error(jsonrpsee::core::Error::RequestTimeout))?;

                            match result.as_ref() {
                                Ok(_) => {
                                    tracer.span_ok();
                                }
                                Err(err) => {
                                    tracer.span_error(format!("{:?}", err));
                                }
                            };

                            result
                        }
                        .with_context(tracer.context(name))
                    },
                )?;
            }

            // register aliases from config
            for (alias_old, alias_new) in config.rpcs.aliases {
                let alias_old = string_to_static_str(alias_old);
                let alias_new = string_to_static_str(alias_new);
                module.register_alias(alias_new, alias_old)?;
            }

            let mut rpc_methods = module.method_names().map(|x| x.to_owned()).collect::<Vec<_>>();

            rpc_methods.sort();

            module.register_method("rpc_methods", move |_, _| {
                Ok::<JsonValue, ErrorObjectOwned>(json!({
                    "version": 1,
                    "methods": rpc_methods
                }))
            })?;

            Ok(module)
        })
        .await?;

    Ok(SubwayServerHandle {
        addr,
        handle,
        extensions: extensions_registry,
    })
}

#[cfg(test)]
mod tests {
    use jsonrpsee::{
        core::client::ClientT,
        rpc_params,
        server::ServerBuilder,
        server::ServerHandle,
        ws_client::{WsClient, WsClientBuilder},
        RpcModule,
    };

    use super::*;
    use crate::{
        config::{MiddlewaresConfig, RpcDefinitions, RpcMethod},
        extensions::{client::ClientConfig, server::ServerConfig, ExtensionsConfig},
    };

    const TIMEOUT: &str = "call_timeout";
    const CRAZY: &str = "go_crazy";
    const PHO: &str = "call_pho";
    const BAR: &str = "bar";

    async fn subway_server(endpoint: String, port: u16, request_timeout_seconds: Option<u64>) -> SubwayServerHandle {
        let config = Config {
            extensions: ExtensionsConfig {
                client: Some(ClientConfig {
                    endpoints: vec![endpoint],
                    shuffle_endpoints: false,
                }),
                server: Some(ServerConfig {
                    listen_address: "127.0.0.1".to_string(),
                    port,
                    max_connections: 1024,
                    request_timeout_seconds: request_timeout_seconds.unwrap_or(10),
                    http_methods: Vec::new(),
                    cors: None,
                }),
                ..Default::default()
            },
            middlewares: MiddlewaresConfig {
                methods: vec!["crazy".to_string(), "upstream".to_string()],
                subscriptions: vec![],
            },
            rpcs: RpcDefinitions {
                methods: vec![
                    RpcMethod {
                        method: PHO.to_string(),
                        params: vec![],
                        cache: None,
                        response: None,
                        delay_ms: None,
                    },
                    RpcMethod {
                        method: TIMEOUT.to_string(),
                        params: vec![],
                        cache: None,
                        response: None,
                        delay_ms: None,
                    },
                    RpcMethod {
                        method: CRAZY.to_string(),
                        params: vec![],
                        cache: None,
                        response: None,
                        delay_ms: None,
                    },
                ],
                subscriptions: vec![],
                aliases: vec![],
            },
        };
        build(config).await.unwrap()
    }

    async fn upstream_dummy_server(url: &str) -> (String, ServerHandle) {
        let server = ServerBuilder::default()
            .max_request_body_size(u32::MAX)
            .max_response_body_size(u32::MAX)
            .max_connections(10 * 1024)
            .build(url)
            .await
            .unwrap();

        let mut module = RpcModule::new(());
        module
            .register_method(PHO, |_, _| Ok::<String, ErrorObjectOwned>(BAR.to_string()))
            .unwrap();
        module
            .register_async_method(TIMEOUT, |_, _| async {
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            })
            .unwrap();
        let addr = format!("ws://{}", server.local_addr().unwrap());
        let handle = server.start(module);
        (addr, handle)
    }

    async fn ws_client(url: &str) -> WsClient {
        WsClientBuilder::default()
            .request_timeout(std::time::Duration::from_secs(60))
            .max_request_size(u32::MAX)
            .max_concurrent_requests(1024 * 1024)
            .build(url)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn null_param_works() {
        let (endpoint, upstream_dummy_server_handle) = upstream_dummy_server("127.0.0.1:9955").await;
        let subway_server = subway_server(endpoint, 9944, None).await;
        let url = format!("ws://{}", subway_server.addr);
        let client = ws_client(&url).await;
        assert_eq!(BAR, client.request::<String, _>(PHO, rpc_params!()).await.unwrap());
        subway_server.handle.stop().unwrap();
        upstream_dummy_server_handle.stop().unwrap();
    }

    #[tokio::test]
    async fn request_timeout() {
        let (endpoint, upstream_dummy_server_handle) = upstream_dummy_server("127.0.0.1:9956").await;
        // server with 1 second timeout
        let subway_server = subway_server(endpoint, 9945, Some(1)).await;
        let url = format!("ws://{}", subway_server.addr);
        // client with default 60 second timeout
        let client = ws_client(&url).await;

        // timeout when middleware goes crazy
        {
            let now = std::time::Instant::now();
            let err = client.request::<String, _>(CRAZY, rpc_params!()).await.unwrap_err();
            // should timeout in 1 second
            assert_eq!(now.elapsed().as_secs(), 1);
            assert!(err.to_string().contains("Request timeout"));
        }

        // timeout when request takes too long
        {
            let now = std::time::Instant::now();
            let err = client.request::<String, _>(TIMEOUT, rpc_params!()).await.unwrap_err();
            // should timeout in 1 second
            assert_eq!(now.elapsed().as_secs(), 1);
            assert!(err.to_string().contains("Request timeout"));
        }

        subway_server.handle.stop().unwrap();
        upstream_dummy_server_handle.stop().unwrap();
    }
}
