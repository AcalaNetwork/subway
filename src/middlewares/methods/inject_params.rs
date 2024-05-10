use async_trait::async_trait;
use jsonrpsee::core::JsonValue;
use opentelemetry::trace::FutureExt;
use std::sync::Arc;

use crate::{
    config::MethodParam,
    extensions::api::{SubstrateApi, ValueHandle},
    middlewares::{
        methods::cache::BypassCache, CallRequest, CallResult, Middleware, MiddlewareBuilder, NextFn, RpcMethod, TRACER,
    },
    utils::errors,
    utils::{TypeRegistry, TypeRegistryRef},
};

pub enum InjectType {
    BlockHashAt(usize),
    BlockNumberAt(usize),
}

pub struct InjectParamsMiddleware {
    head: ValueHandle<(JsonValue, u64)>,
    finalized: ValueHandle<(JsonValue, u64)>,
    inject: InjectType,
    params: Vec<MethodParam>,
}

fn inject_type(params: &[MethodParam]) -> Option<InjectType> {
    let maybe_block_num = params.iter().position(|p| p.inject && p.ty == "BlockNumber");
    if let Some(block_num) = maybe_block_num {
        return Some(InjectType::BlockNumberAt(block_num));
    }

    let maybe_block_hash = params.iter().position(|p| p.inject && p.ty == "BlockHash");
    if let Some(block_hash) = maybe_block_hash {
        return Some(InjectType::BlockHashAt(block_hash));
    }

    None
}

#[async_trait]
impl MiddlewareBuilder<RpcMethod, CallRequest, CallResult> for InjectParamsMiddleware {
    async fn build(
        method: &RpcMethod,
        extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
        let inject_type = inject_type(&method.params)?;

        let api = extensions
            .read()
            .await
            .get::<SubstrateApi>()
            .expect("SubstrateApi extension not found");

        Some(Box::new(Self::new(api, inject_type, method.params.clone())))
    }
}

impl InjectParamsMiddleware {
    pub fn new(api: Arc<SubstrateApi>, inject: InjectType, params: Vec<MethodParam>) -> Self {
        Self {
            head: api.get_head(),
            finalized: api.get_finalized_head(),
            inject,
            params,
        }
    }

    fn get_index(&self) -> usize {
        match self.inject {
            InjectType::BlockHashAt(index) => index,
            InjectType::BlockNumberAt(index) => index,
        }
    }

    async fn get_parameter(&self) -> JsonValue {
        let res = self.head.read().await;
        match self.inject {
            InjectType::BlockHashAt(_) => res.0,
            InjectType::BlockNumberAt(_) => res.1.into(),
        }
    }

    pub fn params_count(&self) -> (usize, usize) {
        let mut optional = 0;
        let mut required = 0;
        for param in &self.params {
            if param.optional {
                optional += 1;
            } else {
                required += 1;
            }
        }
        (required, optional)
    }
}

#[async_trait]
impl Middleware<CallRequest, CallResult> for InjectParamsMiddleware {
    async fn call(
        &self,
        mut request: CallRequest,
        mut context: TypeRegistry,
        next: NextFn<CallRequest, CallResult>,
    ) -> CallResult {
        let handle_request = |request: CallRequest| async {
            for (idx, param) in self.params.iter().enumerate() {
                if param.ty == "BlockNumber" {
                    if let Some(number) = request.params.get(idx).and_then(|x| x.as_u64()) {
                        let (_, finalized) = self.finalized.read().await;
                        // avoid cache unfinalized data
                        if number > finalized {
                            context.insert(BypassCache(true));
                        }
                    }
                }
            }
            next(request, context).await
        };

        let idx = self.get_index();
        let min_len = idx + 1;
        let len = request.params.len();

        match len.cmp(&min_len) {
            std::cmp::Ordering::Greater => {
                // too many params, no injection needed
                return handle_request(request).await;
            }
            std::cmp::Ordering::Less => {
                // too few params

                // ensure missing params are optional and push null
                for i in len..(min_len - 1) {
                    if self.params[i].optional {
                        request.params.push(JsonValue::Null);
                    } else {
                        let (required, optional) = self.params_count();
                        return Err(errors::invalid_params(format!(
                            "Expected {:?} parameters ({:?} optional), {:?} found instead",
                            required + optional,
                            optional,
                            len
                        )));
                    }
                }

                // Set param to null, it will be replaced later
                request.params.push(JsonValue::Null);
            }
            std::cmp::Ordering::Equal => {
                // same number of params, no need to push params
            }
        };

        // Here we are sure we have full params in the request, but it still might be set to null
        async move {
            if request.params[idx].is_null() {
                let to_inject = self.get_parameter().await;
                tracing::trace!("Injected param {} to method {}", &to_inject, request.method);
                request.params[idx] = to_inject;
            }
            handle_request(request).await
        }
        .with_context(TRACER.context("inject_params"))
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::extensions::api::SubstrateApi;
    use crate::extensions::client::mock::{MockRequest, MockSubscription};
    use crate::extensions::client::{mock::TestServerBuilder, Client};
    use futures::FutureExt;
    use jsonrpsee::{server::ServerHandle, SubscriptionMessage, SubscriptionSink};
    use serde_json::json;
    use std::time::Duration;
    use tokio::sync::mpsc;

    struct ExecutionContext {
        api: Arc<SubstrateApi>,
        _server: ServerHandle,
        head_rx: mpsc::Receiver<MockSubscription>,
        finalized_head_rx: mpsc::Receiver<MockSubscription>,
        block_hash_rx: mpsc::Receiver<MockRequest>,
        head_sink: Option<SubscriptionSink>,
        finalized_head_sink: Option<SubscriptionSink>,
    }

    fn bypass_cache(context: &TypeRegistry) -> bool {
        context.get::<BypassCache>().map_or(false, |x| x.0)
    }

    async fn create_client() -> ExecutionContext {
        let mut builder = TestServerBuilder::new();

        let head_rx =
            builder.register_subscription("chain_subscribeNewHeads", "chain_newHead", "chain_unsubscribeNewHeads");

        let finalized_head_rx = builder.register_subscription(
            "chain_subscribeFinalizedHeads",
            "chain_finalizedHead",
            "chain_unsubscribeFinalizedHeads",
        );

        let block_hash_rx = builder.register_method("chain_getBlockHash");

        let (addr, _server) = builder.build().await;

        let client = Client::with_endpoints([format!("ws://{addr}")]).unwrap();
        let api = SubstrateApi::new(Arc::new(client), Duration::from_secs(100));

        ExecutionContext {
            api: Arc::new(api),
            _server,
            head_rx,
            finalized_head_rx,
            block_hash_rx,
            head_sink: None,
            finalized_head_sink: None,
        }
    }

    async fn create_inject_middleware(
        inject_type: InjectType,
        params: Vec<MethodParam>,
    ) -> (InjectParamsMiddleware, ExecutionContext) {
        let mut context = create_client().await;

        let head_sub = context.head_rx.recv().await.unwrap();
        head_sub.send(json!({ "number": "0x4321" })).await;

        {
            let req = context.block_hash_rx.recv().await.unwrap();
            req.respond(json!("0xabcd"));
        }

        let finalized_sub = context.finalized_head_rx.recv().await.unwrap();
        finalized_sub.send(json!({ "number": "0x4321" })).await;
        {
            let req = context.block_hash_rx.recv().await.unwrap();
            req.respond(json!("0xabcd"));
        }

        context.head_sink = Some(head_sub.sink);
        context.finalized_head_sink = Some(finalized_sub.sink);

        (
            InjectParamsMiddleware::new(context.api.clone(), inject_type, params),
            context,
        )
    }

    #[tokio::test]
    async fn skip_inject_if_full_params() {
        let params = vec![json!("0x1234"), json!("0x5678")];
        let (middleware, _) = create_inject_middleware(
            InjectType::BlockHashAt(1),
            vec![
                MethodParam {
                    name: "key".to_string(),
                    ty: "StorageKey".to_string(),
                    optional: false,
                    inject: false,
                },
                MethodParam {
                    name: "at".to_string(),
                    ty: "BlockHash".to_string(),
                    optional: true,
                    inject: true,
                },
            ],
        )
        .await;
        let result = middleware
            .call(
                CallRequest::new("state_getStorage", params.clone()),
                Default::default(),
                Box::new(move |req: CallRequest, _| {
                    async move {
                        assert_eq!(req.params, params);
                        Ok(json!("0x1111"))
                    }
                    .boxed()
                }),
            )
            .await
            .unwrap();
        assert_eq!(result, json!("0x1111"));
    }

    #[tokio::test]
    async fn inject_if_param_is_null() {
        let params = vec![json!("0x1234"), json!(None::<()>)];
        let (middleware, _) = create_inject_middleware(
            InjectType::BlockHashAt(1),
            vec![
                MethodParam {
                    name: "key".to_string(),
                    ty: "StorageKey".to_string(),
                    optional: false,
                    inject: false,
                },
                MethodParam {
                    name: "at".to_string(),
                    ty: "BlockHash".to_string(),
                    optional: true,
                    inject: true,
                },
            ],
        )
        .await;
        let result = middleware
            .call(
                CallRequest::new("state_getStorage", params.clone()),
                Default::default(),
                Box::new(move |req: CallRequest, _| {
                    async move {
                        assert_eq!(req.params, vec![json!("0x1234"), json!("0xabcd")]);
                        Ok(json!("0x1111"))
                    }
                    .boxed()
                }),
            )
            .await
            .unwrap();
        assert_eq!(result, json!("0x1111"));
    }

    #[tokio::test]
    async fn inject_if_without_current_block_hash() {
        let (middleware, _context) = create_inject_middleware(
            InjectType::BlockHashAt(1),
            vec![
                MethodParam {
                    name: "key".to_string(),
                    ty: "StorageKey".to_string(),
                    optional: false,
                    inject: false,
                },
                MethodParam {
                    name: "at".to_string(),
                    ty: "BlockHash".to_string(),
                    optional: true,
                    inject: true,
                },
            ],
        )
        .await;
        let result = middleware
            .call(
                CallRequest::new("state_getStorage", vec![json!("0x1234")]),
                Default::default(),
                Box::new(move |req: CallRequest, _| {
                    async move {
                        assert_eq!(req.params, vec![json!("0x1234"), json!("0xabcd")]);
                        Ok(json!("0x1111"))
                    }
                    .boxed()
                }),
            )
            .await
            .unwrap();
        assert_eq!(result, json!("0x1111"));
    }

    #[tokio::test]
    async fn inject_null_if_expected_optional_param() {
        let (middleware, _context) = create_inject_middleware(
            InjectType::BlockHashAt(2),
            vec![
                MethodParam {
                    name: "key".to_string(),
                    ty: "StorageKey".to_string(),
                    optional: false,
                    inject: false,
                },
                MethodParam {
                    name: "pho".to_string(),
                    ty: "u32".to_string(),
                    optional: true,
                    inject: false,
                },
                MethodParam {
                    name: "at".to_string(),
                    ty: "BlockHash".to_string(),
                    optional: true,
                    inject: true,
                },
            ],
        )
        .await;
        let result = middleware
            .call(
                CallRequest::new("state_getStorage", vec![json!("0x1234")]),
                Default::default(),
                Box::new(move |req: CallRequest, _| {
                    async move {
                        assert_eq!(req.params, vec![json!("0x1234"), JsonValue::Null, json!("0xabcd")]);
                        Ok(json!("0x1111"))
                    }
                    .boxed()
                }),
            )
            .await
            .unwrap();
        assert_eq!(result, json!("0x1111"));
    }

    #[tokio::test]
    async fn err_if_missing_param() {
        let (middleware, _context) = create_inject_middleware(
            InjectType::BlockHashAt(2),
            vec![
                MethodParam {
                    name: "key".to_string(),
                    ty: "StorageKey".to_string(),
                    optional: false,
                    inject: false,
                },
                MethodParam {
                    name: "foo".to_string(),
                    ty: "u32".to_string(),
                    optional: false,
                    inject: false,
                },
                MethodParam {
                    name: "at".to_string(),
                    ty: "BlockHash".to_string(),
                    optional: true,
                    inject: true,
                },
            ],
        )
        .await;
        let result = middleware
            .call(
                CallRequest::new("state_getStorage", vec![json!("0x1234")]),
                Default::default(),
                Box::new(move |req: CallRequest, _| {
                    async move {
                        assert_eq!(req.params, vec![json!("0x1234"), JsonValue::Null, json!("0xabcd")]);
                        Ok(json!("0x1111"))
                    }
                    .boxed()
                }),
            )
            .await;
        assert_eq!(
            result,
            Err(errors::invalid_params(
                "Expected 3 parameters (1 optional), 1 found instead"
            ))
        );
    }

    #[tokio::test]
    async fn inject_if_without_current_block_num() {
        let (middleware, mut context) = create_inject_middleware(
            InjectType::BlockNumberAt(1),
            vec![
                MethodParam {
                    name: "key".to_string(),
                    ty: "StorageKey".to_string(),
                    optional: false,
                    inject: false,
                },
                MethodParam {
                    name: "at".to_string(),
                    ty: "BlockNumber".to_string(),
                    optional: true,
                    inject: true,
                },
            ],
        )
        .await;
        let result = middleware
            .call(
                CallRequest::new("state_getStorage", vec![json!("0x1234")]),
                Default::default(),
                Box::new(move |req: CallRequest, _| {
                    async move {
                        assert_eq!(req.params, vec![json!("0x1234"), json!(0x4321)]);
                        Ok(json!("0x1111"))
                    }
                    .boxed()
                }),
            )
            .await
            .unwrap();
        assert_eq!(result, json!("0x1111"));

        // head updated
        context
            .head_sink
            .unwrap()
            .send(SubscriptionMessage::from_json(&json!({ "number": "0x5432" })).unwrap())
            .await
            .unwrap();
        {
            let req = context.block_hash_rx.recv().await.unwrap();
            req.respond(json!("0xbcde"));
        }

        // finalized updated
        context
            .finalized_head_sink
            .unwrap()
            .send(SubscriptionMessage::from_json(&json!({ "number": "0x5432" })).unwrap())
            .await
            .unwrap();
        {
            let req = context.block_hash_rx.recv().await.unwrap();
            req.respond(json!("0xbcde"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;

        let result2 = middleware
            .call(
                CallRequest::new("state_getStorage", vec![json!("0x1234")]),
                Default::default(),
                Box::new(move |req: CallRequest, _| {
                    async move {
                        assert_eq!(req.params, vec![json!("0x1234"), json!(0x5432)]);
                        Ok(json!("0x1111"))
                    }
                    .boxed()
                }),
            )
            .await
            .unwrap();
        assert_eq!(result2, json!("0x1111"));
    }

    #[tokio::test]
    async fn skip_cache_if_block_number_not_finalized() {
        let (middleware, mut context) = create_inject_middleware(
            InjectType::BlockNumberAt(1),
            vec![
                MethodParam {
                    name: "key".to_string(),
                    ty: "StorageKey".to_string(),
                    optional: false,
                    inject: false,
                },
                MethodParam {
                    name: "at".to_string(),
                    ty: "BlockNumber".to_string(),
                    optional: true,
                    inject: true,
                },
            ],
        )
        .await;

        // head is finalized, cache should not be skipped
        {
            let result = middleware
                .call(
                    CallRequest::new("state_getStorage", vec![json!("0x1234")]),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache not bypassed
                            assert!(!bypass_cache(&context));
                            // block number is not finalized
                            assert_eq!(req.params, vec![json!("0x1234"), json!(0x4321)]);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap();
            assert_eq!(result, json!("0x1111"));
        }

        // block head is updated but not finalized, cache should be skipped
        {
            // head updated but not finalized
            context
                .head_sink
                .unwrap()
                .send(SubscriptionMessage::from_json(&json!({ "number": "0x5432" })).unwrap())
                .await
                .unwrap();
            {
                let req = context.block_hash_rx.recv().await.unwrap();
                req.respond(json!("0xbcde"));
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;

            let result = middleware
                .call(
                    CallRequest::new("state_getStorage", vec![json!("0x1234")]),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache bypassed
                            assert!(bypass_cache(&context));
                            // block number is injected
                            assert_eq!(req.params, vec![json!("0x1234"), json!(0x5432)]);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap();
            assert_eq!(result, json!("0x1111"));
        }

        // request with head block number should skip cache
        {
            let result = middleware
                .call(
                    CallRequest::new("state_getStorage", vec![json!("0x1234"), json!(0x5432)]),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache bypassed
                            assert!(bypass_cache(&context));
                            // params not changed
                            assert_eq!(req.params, vec![json!("0x1234"), json!(0x5432)]);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap();
            assert_eq!(result, json!("0x1111"));
        }

        // request with finalized block number should not skip cache
        {
            let result = middleware
                .call(
                    CallRequest::new("state_getStorage", vec![json!("0x1234"), json!(0x4321)]),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache not bypassed
                            assert!(!bypass_cache(&context));
                            // params not changed
                            assert_eq!(req.params, vec![json!("0x1234"), json!(0x4321)]);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap();
            assert_eq!(result, json!("0x1111"));
        }

        // request with wrong params count will be handled
        {
            // block is finalized, cache should not be skipped
            let result = middleware
                .call(
                    CallRequest::new(
                        "state_getStorage",
                        vec![json!("0x1234"), json!(0x4321), json!("0xabcd")],
                    ),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache not bypassed
                            assert!(!bypass_cache(&context));
                            // params not changed
                            assert_eq!(req.params, vec![json!("0x1234"), json!(0x4321), json!("0xabcd")]);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap();
            assert_eq!(result, json!("0x1111"));

            // block is not finalized, cache should be skipped
            let result = middleware
                .call(
                    CallRequest::new(
                        "state_getStorage",
                        vec![json!("0x1234"), json!(0x5432), json!("0xabcd")],
                    ),
                    Default::default(),
                    Box::new(move |req: CallRequest, context| {
                        async move {
                            // cache bypassed
                            assert!(bypass_cache(&context));
                            // params not changed
                            assert_eq!(req.params, vec![json!("0x1234"), json!(0x5432), json!("0xabcd")]);
                            Ok(json!("0x1111"))
                        }
                        .boxed()
                    }),
                )
                .await
                .unwrap();
            assert_eq!(result, json!("0x1111"));
        }
    }
}
