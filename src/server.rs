use std::{net::SocketAddr, sync::Arc};

use futures::FutureExt;
use jsonrpsee::{
    core::JsonValue,
    server::{RpcModule, ServerHandle},
};
use opentelemetry::trace::FutureExt as _;

use crate::{
    config::Config,
    extensions::server::Server,
    middleware::Middlewares,
    middlewares::{
        create_method_middleware, create_subscription_middleware, CallRequest, SubscriptionRequest,
    },
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

                for ref middleware_name in &middlewares.methods {
                    if let Some(middleware) =
                        create_method_middleware(middleware_name, &method, &extensions).await
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
                        let cx = tracer.context(method_name);

                        let parsed = params.parse::<JsonValue>()?;
                        let params = if parsed == JsonValue::Null {
                            vec![]
                        } else {
                            parsed
                                .as_array()
                                .ok_or_else(|| errors::invalid_params(""))?
                                .to_owned()
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

                for ref middleware_name in &middlewares.subscriptions {
                    if let Some(middleware) =
                        create_subscription_middleware(middleware_name, &subscription, &extensions)
                            .await
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
                    move |params, sink, _| {
                        let subscription_middlewares = subscription_middlewares.clone();

                        async move {
                            let cx = tracer.context(name);

                            let parsed = params.parse::<JsonValue>()?;
                            let params = if parsed == JsonValue::Null {
                                vec![]
                            } else {
                                parsed
                                    .as_array()
                                    .ok_or_else(|| errors::invalid_params(""))?
                                    .to_owned()
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
                    },
                )?;
            }

            Ok(module)
        })
        .await?;

    Ok(res)
}
