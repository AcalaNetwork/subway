use async_trait::async_trait;
use jsonrpsee::{
    core::{Error, JsonValue},
    types::error::CallError,
};
use std::sync::Arc;

use super::{Middleware, NextFn};
use crate::{api::Api, config::MethodParam, middleware::call::CallRequest};

pub enum InjectType {
    BlockHashAt(usize),
    BlockNumberAt(usize),
    BlockTagAt(usize),
}

pub struct InjectParamsMiddleware {
    api: Arc<Api>,
    inject: InjectType,
    params: Vec<MethodParam>,
}

impl InjectParamsMiddleware {
    pub fn new(api: Arc<Api>, inject: InjectType, params: Vec<MethodParam>) -> Self {
        Self {
            api,
            inject,
            params,
        }
    }

    async fn replace_parameter(&self, request: &mut CallRequest) -> Option<(usize, JsonValue)> {
        let (_, number) = self.api.get_head().read().await;
        let finalized_head = self.api.finalized_head.borrow().clone();
        let maybe_inject = match self.inject {
            InjectType::BlockTagAt(index) => {
                if let Some(param) = request.params.get(index).cloned() {
                    if !param.is_string() {
                        return None;
                    }
                    match param.as_str().unwrap_or_default() {
                        "finalized" => {
                            if let Some((_, finalized_number)) = finalized_head {
                                Some((index, format!("0x{:x}", finalized_number).into()))
                            } else {
                                // cannot determine finalized
                                request.bypass_cache = true;
                                None
                            }
                        }
                        "latest" => Some((index, format!("0x{:x}", number).into())),
                        "earliest" => None, // no need to replace earliest because it's always going to be genesis
                        "pending" | "safe" => {
                            request.bypass_cache = true;
                            None
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some((index, to_inject)) = maybe_inject.clone() {
            request.params.remove(index);
            request.params.insert(index, to_inject);
        }

        maybe_inject
    }

    async fn get_parameter(&self) -> JsonValue {
        let (hash, number) = self.api.get_head().read().await;
        match self.inject {
            InjectType::BlockHashAt(_) => hash,
            InjectType::BlockNumberAt(_) => number.into(),
            InjectType::BlockTagAt(_) => JsonValue::Null,
        }
    }
}

pub fn inject(params: &[MethodParam]) -> Option<InjectType> {
    for (index, param) in params.iter().enumerate() {
        if !param.inject {
            continue;
        }
        match param.ty.as_str() {
            "BlockNumber" => return Some(InjectType::BlockNumberAt(index)),
            "BlockHash" => return Some(InjectType::BlockHashAt(index)),
            "BlockTag" => return Some(InjectType::BlockTagAt(index)),
            _ => continue,
        }
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
        let mut reached_optional = false;
        for (index, param) in self.params.iter().enumerate() {
            if !param.optional {
                if reached_optional {
                    return Err(Error::Call(CallError::InvalidParams(anyhow::Error::msg(
                        format!(
                            "config error, cannot have required at {} after optional",
                            index
                        ),
                    ))));
                }
                if let Some(param) = request.params.get(index) {
                    if param.is_null() {
                        return Err(Error::Call(CallError::InvalidParams(anyhow::Error::msg(
                            format!("required param at {} is null", index),
                        ))));
                    }
                } else {
                    return Err(Error::Call(CallError::InvalidParams(anyhow::Error::msg(
                        format!("missing required param at {}", index),
                    ))));
                }
            }
            if param.inject {
                let injected = if param.ty == "BlockTag" {
                    self.replace_parameter(&mut request).await
                } else if param.optional && request.params.get(index).is_none() {
                    let to_inject = self.get_parameter().await;
                    request.params.push(to_inject.clone());
                    Some((index, to_inject))
                } else {
                    None
                };
                log::debug!("Injected {:?} to request: {:?}", injected, request);
            } else if param.optional {
                reached_optional = true;
                request.params.insert(index, JsonValue::Null);
            }
        }

        next(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::client::{mock::TestServerBuilder, Client};
    use futures::FutureExt;
    use jsonrpsee::{server::ServerHandle, SubscriptionMessage, SubscriptionSink};
    use serde_json::json;
    use std::time::Duration;
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
        let api = Api::new(Arc::new(client), Duration::from_secs(100), false);

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
                CallRequest::new("state_getStorage", params.clone()),
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
                CallRequest::new("state_getStorage", vec![json!("0x1234")]),
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
                CallRequest::new("state_getStorage", vec![json!("0x1234")]),
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
                CallRequest::new("state_getStorage", vec![json!("0x1234")]),
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
            matches!(result, Err(Error::Call(CallError::InvalidParams(e))) if e.to_string() == "missing required param at 1")
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
                CallRequest::new("state_getStorage", vec![json!("0x1234")]),
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
