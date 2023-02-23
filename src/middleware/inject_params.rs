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

mod tests {
    use super::*;

    use crate::client::{mock::TestServerBuilder, Client};
    use futures::FutureExt;
    use jsonrpsee::{server::ServerHandle, SubscriptionMessage, SubscriptionSink};
    use serde_json::json;
    use tokio::sync::{mpsc, oneshot};

    async fn create_client() -> (
        Api,
        ServerHandle,
        mpsc::Receiver<(JsonValue, SubscriptionSink)>,
        mpsc::Receiver<(JsonValue, SubscriptionSink)>,
        mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
    ) {
        let mut builder = TestServerBuilder::new();

        let head_rx = builder.register_subscription(
            "chain_subscribeNewHeads",
            "chain_newHead",
            "chain_unsubscribeNewHeads",
        );

        let finalized_head_rx = builder.register_subscription(
            "chain_subscribeFinalizedHeads",
            "chain_finalizedHead",
            "chain_unsubscribeFinalizedHeads",
        );

        let block_hash_rx = builder.register_method("chain_getBlockHash");

        let (addr, server) = builder.build().await;

        let client = Client::new(&[format!("ws://{addr}")]).await.unwrap();
        let api = Api::new(Arc::new(client));

        (api, server, head_rx, finalized_head_rx, block_hash_rx)
    }

    async fn create_inject_middleware() -> InjectParamsMiddleware {
        let (api, _server, mut head_rx, _finalized_head_rx, mut block_rx) = create_client().await;

        let (_, head_sink) = head_rx.recv().await.unwrap();
        head_sink
            .send(SubscriptionMessage::from_json(&json!({ "number": "0x4321" })).unwrap())
            .await
            .unwrap();

        {
            let (_, tx) = block_rx.recv().await.unwrap();
            tx.send(json!("0xabcd")).unwrap();
        }

        InjectParamsMiddleware::new(
            Arc::new(api),
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
    }

    #[tokio::test]
    async fn skip_inject_if_full_params() {
        let params = vec![json!("0x1234"), json!("0x5678")];
        let middleware = create_inject_middleware().await;
        middleware
            .call(
                CallRequest {
                    method: "state_getStorage".to_string(),
                    params: params.clone(),
                },
                Box::new(move |req: CallRequest| {
                    async move {
                        assert_eq!(req.params, params);
                        Ok(json!("0x1234"))
                    }
                    .boxed()
                }),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn inject_if_without_current_block() {
        let middleware = create_inject_middleware().await;
        middleware
            .call(
                CallRequest {
                    method: "state_getStorage".to_string(),
                    params: vec![json!("0x1234")],
                },
                Box::new(move |req: CallRequest| {
                    async move {
                        assert_eq!(req.params, vec![json!("0x1234"), json!("0xabcd")]);
                        Ok(json!("0x1234"))
                    }
                    .boxed()
                }),
            )
            .await
            .unwrap();
    }
}
