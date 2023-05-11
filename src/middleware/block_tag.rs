use async_trait::async_trait;
use jsonrpsee::{core::JsonValue, types::ErrorObjectOwned};
use std::sync::Arc;
use tracing::instrument;

use super::{Middleware, NextFn};
use crate::{api::EthApi, middleware::call::CallRequest};

pub struct BlockTagMiddleware {
    api: Arc<EthApi>,
    index: usize,
}

impl BlockTagMiddleware {
    pub fn new(api: Arc<EthApi>, index: usize) -> Self {
        Self { api, index }
    }

    async fn replace(&self, mut request: CallRequest) -> CallRequest {
        let maybe_value = {
            if let Some(param) = request.params.get(self.index).cloned() {
                if !param.is_string() {
                    // nothing to do here
                    return request;
                }
                match param.as_str().unwrap_or_default() {
                    "finalized" => {
                        let finalized_head = self.api.current_finalized_head();
                        if let Some((_, finalized_number)) = finalized_head {
                            Some(format!("0x{:x}", finalized_number).into())
                        } else {
                            // cannot determine finalized
                            request.extra.bypass_cache = true;
                            None
                        }
                    }
                    "latest" => {
                        let (_, number) = self.api.get_head().read().await;
                        Some(format!("0x{:x}", number).into())
                    }
                    "earliest" => None, // no need to replace earliest because it's always going to be genesis
                    "pending" | "safe" => {
                        request.extra.bypass_cache = true;
                        None
                    }
                    _ => None,
                }
            } else {
                None
            }
        };

        if let Some(value) = maybe_value {
            log::debug!(
                "Replacing params {:?} updated with {:?}",
                request.params,
                (self.index, &value),
            );
            request.params.remove(self.index);
            request.params.insert(self.index, value);
        }

        request
    }
}

#[async_trait]
impl Middleware<CallRequest, Result<JsonValue, ErrorObjectOwned>> for BlockTagMiddleware {
    #[instrument(skip_all)]
    async fn call(
        &self,
        request: CallRequest,
        next: NextFn<CallRequest, Result<JsonValue, ErrorObjectOwned>>,
    ) -> Result<JsonValue, ErrorObjectOwned> {
        let request = self.replace(request).await;
        next(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::api::EthApi;
    use crate::client::mock::{run_sink_tasks, SinkTask};
    use crate::client::{mock::TestServerBuilder, Client};
    use crate::config::MethodParam;
    use futures::FutureExt;
    use jsonrpsee::{server::ServerHandle, SubscriptionSink};
    use serde_json::json;
    use std::time::Duration;
    use tokio::sync::{mpsc, oneshot};

    struct ExecutionContext {
        _server: ServerHandle,
        subscribe_rx: mpsc::Receiver<(JsonValue, SubscriptionSink)>,
        get_block_rx: mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
    }

    impl ExecutionContext {
        async fn send_current_block(&mut self, msg: JsonValue) {
            let (_, tx) = self.get_block_rx.recv().await.unwrap();
            tx.send(msg).unwrap();
        }
    }

    async fn create_client() -> (ExecutionContext, EthApi) {
        let mut builder = TestServerBuilder::new();

        let subscribe_rx =
            builder.register_subscription("eth_subscribe", "eth_subscription", "eth_unsubscribe");

        let get_block_rx = builder.register_method("eth_getBlockByNumber");

        let (addr, _server) = builder.build().await;

        let client = Client::new(&[format!("ws://{addr}")]).await.unwrap();
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

    async fn create_block_tag_middleware(
        params: Vec<MethodParam>,
    ) -> (BlockTagMiddleware, ExecutionContext) {
        let (context, api) = create_client().await;

        (
            BlockTagMiddleware::new(
                Arc::new(api),
                params.iter().position(|p| p.ty == "BlockTag").unwrap(),
            ),
            context,
        )
    }

    #[tokio::test]
    async fn skip_replacement_if_no_tag() {
        let params = vec![json!("0x1234"), json!("0x5678")];
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
                    Box::new(move |req: CallRequest| {
                        async move {
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
            let (value, subscribe_sink) = context.subscribe_rx.recv().await.unwrap();
            if value
                .as_array()
                .unwrap()
                .contains(&json!("newFinalizedHeads"))
            {
                run_sink_tasks(
                    &subscribe_sink,
                    vec![SinkTask::Send(
                        json!({ "number": "0x5430", "hash": "0x00" }),
                    )],
                )
                .await
            }

            let (value, subscribe_sink) = context.subscribe_rx.recv().await.unwrap();
            if value.as_array().unwrap().contains(&json!("newHeads")) {
                run_sink_tasks(
                    &subscribe_sink,
                    vec![SinkTask::Send(
                        json!({ "number": "0x5432", "hash": "0x02" }),
                    )],
                )
                .await
            }
        });

        assert_eq!(
            middleware
                .call(
                    CallRequest::new("state_getStorage", vec![json!("0x1234"), json!("latest")]),
                    Box::new(move |req: CallRequest| {
                        async move {
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
                    CallRequest::new(
                        "state_getStorage",
                        vec![json!("0x1234"), json!("finalized")],
                    ),
                    Box::new(move |req: CallRequest| {
                        async move {
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
                    CallRequest::new(
                        "state_getStorage",
                        vec![json!("0x1234"), json!("finalized")],
                    ),
                    Box::new(move |req: CallRequest| {
                        async move {
                            assert_eq!(req.params, vec![json!("0x1234"), json!("0x5430")]);
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
                    Box::new(move |req: CallRequest| {
                        async move {
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
