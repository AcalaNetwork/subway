use std::{net::SocketAddr, sync::Arc};

use futures::FutureExt;
use jsonrpsee::{
    core::JsonValue,
    server::{RpcModule, ServerHandle},
    types::ErrorObjectOwned,
};
use opentelemetry::trace::FutureExt as _;
use serde_json::json;

use crate::{
    config::Config,
    extensions::server::Server,
    middleware::Middlewares,
    middlewares::{create_method_middleware, create_subscription_middleware, CallRequest, SubscriptionRequest},
    utils::{errors, telemetry},
};

// TODO: https://github.com/paritytech/jsonrpsee/issues/985
fn string_to_static_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

pub async fn start_server(config: Config) -> anyhow::Result<(SocketAddr, ServerHandle)> {
    let Config {
        extensions,
        middlewares,
        rpcs,
    } = config;

    let extensions = extensions
        .create_registry()
        .await
        .expect("Failed to create extensions registry");

    let server = extensions
        .read()
        .await
        .get::<Server>()
        .expect("Server extension not found");

    let res = server
        .create_server(move || async move {
            let mut module = RpcModule::new(());

            let tracer = telemetry::Tracer::new("server");

            for method in rpcs.methods {
                let mut method_middlewares: Vec<Arc<_>> = vec![];

                for middleware_name in &middlewares.methods {
                    if let Some(middleware) = create_method_middleware(middleware_name, &method, &extensions).await {
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
                        let cx = tracer.context(method_name);

                        let parsed = params.parse::<JsonValue>()?;
                        let params = if parsed == JsonValue::Null {
                            vec![]
                        } else {
                            parsed.as_array().ok_or_else(|| errors::invalid_params(""))?.to_owned()
                        };
                        method_middlewares
                            .call(CallRequest::new(method_name, params))
                            .with_context(cx)
                            .await
                    }
                })?;
            }

            for subscription in rpcs.subscriptions {
                let subscribe_name = string_to_static_str(subscription.subscribe.clone());
                let unsubscribe_name = string_to_static_str(subscription.unsubscribe.clone());
                let name = string_to_static_str(subscription.name.clone());

                let mut subscription_middlewares: Vec<Arc<_>> = vec![];

                for middleware_name in &middlewares.subscriptions {
                    if let Some(middleware) =
                        create_subscription_middleware(middleware_name, &subscription, &extensions).await
                    {
                        subscription_middlewares.push(middleware.into());
                    }
                }

                let subscription_middlewares = Middlewares::new(
                    subscription_middlewares,
                    Arc::new(|_, _| async { Err("Bad configuration".into()) }.boxed()),
                );

                module.register_subscription(subscribe_name, name, unsubscribe_name, move |params, sink, _| {
                    let subscription_middlewares = subscription_middlewares.clone();

                    async move {
                        let cx = tracer.context(name);

                        let parsed = params.parse::<JsonValue>()?;
                        let params = if parsed == JsonValue::Null {
                            vec![]
                        } else {
                            parsed.as_array().ok_or_else(|| errors::invalid_params(""))?.to_owned()
                        };
                        subscription_middlewares
                            .call(SubscriptionRequest {
                                subscribe: subscribe_name.into(),
                                params,
                                unsubscribe: unsubscribe_name.into(),
                                sink,
                            })
                            .with_context(cx)
                            .await
                    }
                })?;
            }

            for (alias_old, alias_new) in rpcs.aliases {
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

    Ok(res)
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

    const PHO: &str = "pho";
    const BAR: &str = "bar";
    const WS_SERVER_ENDPOINT: &str = "127.0.0.1:9955";

    async fn server() -> (String, ServerHandle) {
        let config = Config {
            extensions: ExtensionsConfig {
                client: Some(ClientConfig {
                    endpoints: vec![format!("ws://{}", WS_SERVER_ENDPOINT)],
                    shuffle_endpoints: false,
                }),
                server: Some(ServerConfig {
                    listen_address: "127.0.0.1".to_string(),
                    port: 9944,
                    max_connections: 1024,
                    http_methods: Vec::new(),
                }),
                ..Default::default()
            },
            middlewares: MiddlewaresConfig {
                methods: vec!["upstream".to_string()],
                subscriptions: vec![],
            },
            rpcs: RpcDefinitions {
                methods: vec![RpcMethod {
                    method: PHO.to_string(),
                    params: vec![],
                    cache: None,
                    response: None,
                }],
                subscriptions: vec![],
                aliases: vec![],
            },
        };
        let (addr, server) = start_server(config).await.unwrap();
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
            .register_method(PHO, |_, _| Ok::<std::string::String, ErrorObjectOwned>(BAR.to_string()))
            .unwrap();
        let addr = format!("ws://{}", server.local_addr().unwrap());
        let handle = server.start(module);
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
        assert_eq!(BAR, client.request::<String, _>(PHO, rpc_params!()).await.unwrap());
    }
}
