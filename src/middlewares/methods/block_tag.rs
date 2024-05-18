use std::sync::Arc;

use async_trait::async_trait;
use opentelemetry::trace::FutureExt;

use crate::{
    extensions::api::EthApi,
    middlewares::{CallRequest, CallResult, Middleware, MiddlewareBuilder, NextFn, RpcMethod, TRACER},
    utils::{TypeRegistry, TypeRegistryRef},
};

use super::cache::BypassCache;

pub struct BlockTagMiddleware {
    api: Arc<EthApi>,
    index: usize,
}

#[async_trait]
impl MiddlewareBuilder<RpcMethod, CallRequest, CallResult> for BlockTagMiddleware {
    async fn build(
        method: &RpcMethod,
        extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
        let index = method.params.iter().position(|p| p.ty == "BlockTag" && p.inject)?;

        let eth_api = extensions
            .read()
            .await
            .get::<EthApi>()
            .expect("EthApi extension not found");

        Some(Box::new(BlockTagMiddleware::new(eth_api, index)))
    }
}

impl BlockTagMiddleware {
    pub fn new(api: Arc<EthApi>, index: usize) -> Self {
        Self { api, index }
    }

    async fn replace(&self, mut request: CallRequest, mut context: TypeRegistry) -> (CallRequest, TypeRegistry) {
        let maybe_value = {
            if let Some(param) = request.params.get(self.index).cloned() {
                if !param.is_string() {
                    // nothing to do here
                    return (request, context);
                }
                match param.as_str().unwrap_or_default() {
                    "finalized" => {
                        let finalized_head = self.api.current_finalized_head();
                        if let Some((_, finalized_number)) = finalized_head {
                            Some(format!("0x{:x}", finalized_number).into())
                        } else {
                            // cannot determine finalized
                            context.insert(BypassCache(true));
                            None
                        }
                    }
                    "latest" => {
                        // bypass cache for latest block to avoid caching forks
                        context.insert(BypassCache(true));
                        let (_, number) = self.api.get_head().read().await;
                        Some(format!("0x{:x}", number).into())
                    }
                    "earliest" => None, // no need to replace earliest because it's always going to be genesis
                    "pending" | "safe" => {
                        context.insert(BypassCache(true));
                        None
                    }
                    number => {
                        // bypass cache for block number to avoid caching forks unless it's a finalized block
                        let mut bypass_cache = true;
                        if let Some((_, finalized_number)) = self.api.current_finalized_head() {
                            if let Some(hex_number) = number.strip_prefix("0x") {
                                if let Ok(number) = u64::from_str_radix(hex_number, 16) {
                                    if number <= finalized_number {
                                        bypass_cache = false;
                                    }
                                }
                            }
                        }
                        if bypass_cache {
                            context.insert(BypassCache(true));
                        }
                        None
                    }
                }
            } else {
                None
            }
        };

        if let Some(value) = maybe_value {
            tracing::trace!(
                "Replacing params {:?} updated with {:?}",
                request.params,
                (self.index, &value),
            );
            request.params.remove(self.index);
            request.params.insert(self.index, value);
        }

        (request, context)
    }
}

#[async_trait]
impl Middleware<CallRequest, CallResult> for BlockTagMiddleware {
    async fn call(
        &self,
        request: CallRequest,
        context: TypeRegistry,
        next: NextFn<CallRequest, CallResult>,
    ) -> CallResult {
        async move {
            let (request, context) = self.replace(request, context).await;
            next(request, context).await
        }
        .with_context(TRACER.context("block_tag"))
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::MethodParam;
    use crate::extensions::api::EthApi;
    use crate::extensions::client::mock::{MockRequest, MockSubscription};
    use crate::extensions::client::{
        mock::{SinkTask, TestServerBuilder},
        Client,
    };
    use futures::FutureExt;
    use jsonrpsee::{core::JsonValue, server::ServerHandle};
    use serde_json::json;
    use std::time::Duration;
    use tokio::sync::mpsc;

    struct ExecutionContext {
        _server: ServerHandle,
        subscribe_rx: mpsc::Receiver<MockSubscription>,
        get_block_rx: mpsc::Receiver<MockRequest>,
    }

    impl ExecutionContext {
        async fn send_current_block(&mut self, msg: JsonValue) {
            let req = self.get_block_rx.recv().await.unwrap();
            req.respond(msg);
        }
    }

    fn bypass_cache(context: &TypeRegistry) -> bool {
        context.get::<BypassCache>().map_or(false, |x| x.0)
    }

    async fn create_client() -> (ExecutionContext, EthApi) {
        let mut builder = TestServerBuilder::new();

        let subscribe_rx = builder.register_subscription("eth_subscribe", "eth_subscription", "eth_unsubscribe");

        let get_block_rx = builder.register_method("eth_getBlockByNumber");

        let (addr, _server) = builder.build().await;

        let client = Client::with_endpoints([format!("ws://{addr}")]).unwrap();
        let api = EthApi::new(Arc::new(client), Duration::from_secs(100));

        (
            ExecutionContext {
                _server,
                subscribe_rx,
                get_block_rx,
            },
            api,
        )
    }

    async fn create_block_tag_middleware(params: Vec<MethodParam>) -> (BlockTagMiddleware, ExecutionContext) {
        let (context, api) = create_client().await;

        (
            BlockTagMiddleware::new(Arc::new(api), params.iter().position(|p| p.ty == "BlockTag").unwrap()),
            context,
        )
    }

    #[tokio::test]
    async fn skip_replacement_if_no_tag() {
        let params = vec![json!("0x1234"), json!("0x4321")];
        let (middleware, mut context) = create_block_tag_middleware(vec![
            MethodParam {
                name: "key".to_string(),
                ty: "StorageKey".to_string(),
                optional: false,
                inject: false,
            },
            MethodParam {
                name: "at".to_string(),
                ty: "BlockTag".to_string(),
                optional: false,
                inject: true,
            },
        ])
        .await;

        context
            .send_current_block(json!({ "number": "0x4321", "hash": "0x00" }))
            .await;

        assert_eq!(
            middleware
                .call(
                    CallRequest::new("state_getStorage", params.clone()),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache bypassed, cannot determine finalized block
                            assert!(bypass_cache(&context));
                            // no replacement
                            assert_eq!(req.params, params);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap(),
            json!("0x1111")
        );
    }

    #[tokio::test]
    async fn works() {
        let (middleware, mut context) = create_block_tag_middleware(vec![
            MethodParam {
                name: "key".to_string(),
                ty: "StorageKey".to_string(),
                optional: false,
                inject: false,
            },
            MethodParam {
                name: "at".to_string(),
                ty: "BlockTag".to_string(),
                optional: false,
                inject: true,
            },
        ])
        .await;

        let send_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            context
                .send_current_block(json!({ "number": "0x4321", "hash": "0x01" }))
                .await;

            tokio::time::sleep(Duration::from_millis(10)).await;
            let sub = context.subscribe_rx.recv().await.unwrap();
            if sub.params.as_array().unwrap().contains(&json!("newFinalizedHeads")) {
                sub.run_sink_tasks(vec![SinkTask::Send(json!({ "number": "0x4321", "hash": "0x01" }))])
                    .await
            }

            let sub = context.subscribe_rx.recv().await.unwrap();
            if sub.params.as_array().unwrap().contains(&json!("newHeads")) {
                sub.run_sink_tasks(vec![SinkTask::Send(json!({ "number": "0x5432", "hash": "0x02" }))])
                    .await
            }
        });

        assert_eq!(
            middleware
                .call(
                    CallRequest::new("state_getStorage", vec![json!("0x1234"), json!("latest")]),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache bypassed for latest
                            assert!(bypass_cache(&context));
                            // latest block replaced with block number
                            assert_eq!(req.params, vec![json!("0x1234"), json!("0x4321")]);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap(),
            json!("0x1111")
        );

        assert_eq!(
            middleware
                .call(
                    CallRequest::new("state_getStorage", vec![json!("0x1234"), json!("finalized")],),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache bypassed, block tag not replaced
                            assert!(bypass_cache(&context));
                            // block tag not replaced
                            assert_eq!(req.params, vec![json!("0x1234"), json!("finalized")]);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap(),
            json!("0x1111")
        );

        send_task.await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert_eq!(
            middleware
                .call(
                    CallRequest::new("state_getStorage", vec![json!("0x1234"), json!("finalized")],),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache not bypassed, finalized replaced with block number
                            assert!(!bypass_cache(&context));
                            // block tag replaced with block number
                            assert_eq!(req.params, vec![json!("0x1234"), json!("0x4321")]);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap(),
            json!("0x1111")
        );

        assert_eq!(
            middleware
                .call(
                    CallRequest::new("state_getStorage", vec![json!("0x1234"), json!("latest")]),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache bypassed for latest
                            assert!(bypass_cache(&context));
                            // latest block replaced with block number
                            assert_eq!(req.params, vec![json!("0x1234"), json!("0x5432")]);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap(),
            json!("0x1111")
        );
    }
}
