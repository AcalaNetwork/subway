use async_trait::async_trait;
use jsonrpsee::{
    core::{Error, JsonValue},
    types::error::CallError,
};
use std::sync::Arc;

use super::{Middleware, NextFn};
use crate::{
    api::{Api, ValueHandle},
    config::MethodParam,
    middleware::call::CallRequest,
};

pub enum InjectType {
    BlockHashAt(usize),
    BlockNumberAt(usize),
}

pub struct InjectParamsMiddleware {
    head: ValueHandle<(JsonValue, u64)>,
    inject: InjectType,
    params: Vec<MethodParam>,
}

impl InjectParamsMiddleware {
    pub fn new(api: Arc<Api>, inject: InjectType, params: Vec<MethodParam>) -> Self {
        Self {
            head: api.get_head(),
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

pub fn inject(params: &[MethodParam]) -> Option<InjectType> {
    let maybe_block_num = params
        .iter()
        .position(|p| p.inject && p.ty == "BlockNumber");
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
impl Middleware<CallRequest, Result<JsonValue, Error>> for InjectParamsMiddleware {
    async fn call(
        &self,
        mut request: CallRequest,
        next: NextFn<CallRequest, Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        let idx = self.get_index();
        match request.params.len() {
            len if len == idx + 1 => {
                // full params with current block
                return next(request).await;
            }
            len if len <= idx => {
                // without current block
                let to_inject = self.get_parameter().await;
                tracing::debug!("Injected param {} to method {}", &to_inject, request.method);
                let params_passed = request.params.len();
                while request.params.len() < idx {
                    let current = request.params.len();
                    if self.params[current].optional {
                        request.params.push(JsonValue::Null);
                    } else {
                        let (required, optional) = self.params_count();
                        return Err(Error::Call(CallError::InvalidParams(anyhow::Error::msg(
                            format!(
                                "Expected {:?} parameters ({:?} optional), {:?} found instead",
                                required + optional,
                                optional,
                                params_passed
                            ),
                        ))));
                    }
                }
                request.params.push(to_inject);

                return next(request).await;
            }
            _ => {
                // unexpected number of params
                next(request).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::client::{mock::TestServerBuilder, Client};
    use futures::FutureExt;
    use jsonrpsee::{server::ServerHandle, SubscriptionMessage, SubscriptionSink};
    use serde_json::json;
    use tokio::sync::{mpsc, oneshot};

    struct ExecutionContext {
        _server: ServerHandle,
        head_rx: mpsc::Receiver<(JsonValue, SubscriptionSink)>,
        _finalized_head_rx: mpsc::Receiver<(JsonValue, SubscriptionSink)>,
        block_hash_rx: mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
        head_sink: Option<SubscriptionSink>,
    }

    async fn create_client() -> (ExecutionContext, Api) {
        let mut builder = TestServerBuilder::new();

        let head_rx = builder.register_subscription(
            "chain_subscribeNewHeads",
            "chain_newHead",
            "chain_unsubscribeNewHeads",
        );

        let _finalized_head_rx = builder.register_subscription(
            "chain_subscribeFinalizedHeads",
            "chain_finalizedHead",
            "chain_unsubscribeFinalizedHeads",
        );

        let block_hash_rx = builder.register_method("chain_getBlockHash");

        let (addr, _server) = builder.build().await;

        let client = Client::new(&[format!("ws://{addr}")]).await.unwrap();
        let api = Api::new(Arc::new(client));

        (
            ExecutionContext {
                _server,
                head_rx,
                _finalized_head_rx,
                block_hash_rx,
                head_sink: None,
            },
            api,
        )
    }

    async fn create_inject_middleware(
        inject_type: InjectType,
        params: Vec<MethodParam>,
    ) -> (InjectParamsMiddleware, ExecutionContext) {
        let (mut context, api) = create_client().await;

        let (_, head_sink) = context.head_rx.recv().await.unwrap();
        head_sink
            .send(SubscriptionMessage::from_json(&json!({ "number": "0x4321" })).unwrap())
            .await
            .unwrap();

        {
            let (_, tx) = context.block_hash_rx.recv().await.unwrap();
            tx.send(json!("0xabcd")).unwrap();
        }

        context.head_sink = Some(head_sink);

        (
            InjectParamsMiddleware::new(Arc::new(api), inject_type, params),
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
                CallRequest {
                    method: "state_getStorage".to_string(),
                    params: params.clone(),
                },
                Box::new(move |req: CallRequest| {
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
    async fn inject_if_without_current_block_hash() {
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
                CallRequest {
                    method: "state_getStorage".to_string(),
                    params: vec![json!("0x1234")],
                },
                Box::new(move |req: CallRequest| {
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
        let (middleware, _) = create_inject_middleware(
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
                CallRequest {
                    method: "state_getStorage".to_string(),
                    params: vec![json!("0x1234")],
                },
                Box::new(move |req: CallRequest| {
                    async move {
                        assert_eq!(
                            req.params,
                            vec![json!("0x1234"), JsonValue::Null, json!("0xabcd")]
                        );
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
        let (middleware, _) = create_inject_middleware(
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
                CallRequest {
                    method: "state_getStorage".to_string(),
                    params: vec![json!("0x1234")],
                },
                Box::new(move |req: CallRequest| {
                    async move {
                        assert_eq!(
                            req.params,
                            vec![json!("0x1234"), JsonValue::Null, json!("0xabcd")]
                        );
                        Ok(json!("0x1111"))
                    }
                    .boxed()
                }),
            )
            .await;
        assert!(
            matches!(result, Err(Error::Call(CallError::InvalidParams(e))) if e.to_string() == "Expected 3 parameters (1 optional), 1 found instead")
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
                CallRequest {
                    method: "state_getStorage".to_string(),
                    params: vec![json!("0x1234")],
                },
                Box::new(move |req: CallRequest| {
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
            let (_, tx) = context.block_hash_rx.recv().await.unwrap();
            tx.send(json!("0xbcde")).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;

        let result2 = middleware
            .call(
                CallRequest {
                    method: "state_getStorage".to_string(),
                    params: vec![json!("0x1234")],
                },
                Box::new(move |req: CallRequest| {
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
}
