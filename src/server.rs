use futures::FutureExt;
use jsonrpsee::core::JsonValue;
use jsonrpsee::server::{RandomStringIdProvider, RpcModule, ServerBuilder};
use jsonrpsee::types::error::CallError;
use serde_json::json;
use std::time::Duration;
use std::{net::SocketAddr, num::NonZeroUsize, sync::Arc};
use tokio::task::JoinHandle;

use crate::cache::new_cache;
use crate::{
    api::Api,
    client::Client,
    config::Config,
    middleware::{
        cache::CacheMiddleware,
        call::{self, CallRequest},
        inject_params::{InjectParamsMiddleware, InjectType},
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
) -> anyhow::Result<(SocketAddr, JoinHandle<()>)> {
    let service_builder = tower::ServiceBuilder::new();

    let server = ServerBuilder::default()
        .set_middleware(service_builder)
        .max_connections(config.server.max_connections)
        .set_id_provider(RandomStringIdProvider::new(16))
        .build((config.server.listen_address.as_str(), config.server.port))
        .await?;

    let mut module = RpcModule::new(());

    let client = Arc::new(client);
    let api = Arc::new(Api::new(
        client.clone(),
        Duration::from_secs(config.stale_timeout_seconds),
    ));

    let upstream = Arc::new(call::UpstreamMiddleware::new(client.clone()));

    for method in &config.rpcs.methods {
        let mut list: Vec<Arc<dyn Middleware<_, _>>> = vec![];

        if let Some(hash_index) = method.with_block_hash {
            list.push(Arc::new(InjectParamsMiddleware::new(
                api.clone(),
                InjectType::BlockHashAt(hash_index),
            )));
        }
        if let Some(number_index) = method.with_block_number {
            list.push(Arc::new(InjectParamsMiddleware::new(
                api.clone(),
                InjectType::BlockNumberAt(number_index),
            )));
        }

        if method.cache > 0 {
            // each method has it's own cache
            let cache_size = NonZeroUsize::new(method.cache).expect("qed;");
            let cache = new_cache(cache_size, None);
            list.push(Arc::new(CacheMiddleware::new(cache)));
        }
        list.push(upstream.clone());

        let middlewares = Arc::new(Middlewares::new(
            list,
            Arc::new(|_| {
                async {
                    Err(
                        jsonrpsee::types::error::CallError::Failed(anyhow::Error::msg(
                            "Bad configuration",
                        ))
                        .into(),
                    )
                }
                .boxed()
            }),
        ));

        let method_name = string_to_static_str(method.method.clone());
        module.register_async_method(method_name, move |params, _| {
            let middlewares = middlewares.clone();
            async move {
                let params = params
                    .parse::<JsonValue>()?
                    .as_array()
                    .ok_or_else(|| CallError::InvalidParams(anyhow::Error::msg("invalid params")))?
                    .to_owned();
                middlewares
                    .call(CallRequest::new(method_name.into(), params))
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
            ))),
            None => list.push(upstream.clone()),
        };

        let middlewares = Arc::new(Middlewares::new(
            list,
            Arc::new(|_| {
                async {
                    Err(
                        jsonrpsee::types::error::CallError::Failed(anyhow::Error::msg(
                            "Bad configuration",
                        ))
                        .into(),
                    )
                }
                .boxed()
            }),
        ));

        module.register_subscription(
            subscribe_name,
            name,
            unsubscribe_name,
            move |params, sink, _| {
                let middlewares = middlewares.clone();
                async move {
                    let params = params
                        .parse::<JsonValue>()?
                        .as_array()
                        .ok_or_else(|| {
                            CallError::InvalidParams(anyhow::Error::msg("invalid params"))
                        })?
                        .to_owned();
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
        Ok(json!({
            "version": 1,
            "methods": rpc_methods
        }))
    })?;

    let addr = server.local_addr()?;
    let handle = server.start(module)?;

    let handle = tokio::spawn(handle.stopped());

    Ok((addr, handle))
}
