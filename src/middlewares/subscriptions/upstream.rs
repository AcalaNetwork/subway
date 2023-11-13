use std::sync::Arc;

use async_trait::async_trait;
use jsonrpsee::SubscriptionMessage;

use crate::{
    extensions::client::Client,
    middleware::{Middleware, MiddlewareBuilder, NextFn, RpcSubscription},
    middlewares::{SubscriptionRequest, SubscriptionResult},
    utils::{errors, TypeRegistry, TypeRegistryRef},
};

pub struct UpstreamMiddleware {
    client: Arc<Client>,
}

impl UpstreamMiddleware {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl MiddlewareBuilder<RpcSubscription, SubscriptionRequest, SubscriptionResult> for UpstreamMiddleware {
    async fn build(
        _method: &RpcSubscription,
        extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<SubscriptionRequest, SubscriptionResult>>> {
        let client = extensions
            .read()
            .await
            .get::<Client>()
            .expect("Client extension not found");
        Some(Box::new(UpstreamMiddleware::new(client)))
    }
}

#[async_trait]
impl Middleware<SubscriptionRequest, SubscriptionResult> for UpstreamMiddleware {
    async fn call(
        &self,
        request: SubscriptionRequest,
        _context: TypeRegistry,
        _next: NextFn<SubscriptionRequest, SubscriptionResult>,
    ) -> SubscriptionResult {
        let SubscriptionRequest {
            subscribe,
            params,
            unsubscribe,
            pending_sink,
        } = request;

        let result = self.client.subscribe(&subscribe, params, &unsubscribe).await;

        let (mut subscription, sink) = match result {
            // subscription was successful, accept the sink
            Ok(sub) => match pending_sink.accept().await {
                Ok(sink) => (sub, sink),
                Err(e) => {
                    tracing::trace!("Failed to accept pending subscription {:?}", e);
                    // sink was closed before we could accept it, unsubscribe remote upstream
                    if let Err(err) = sub.unsubscribe().await {
                        tracing::error!("Failed to unsubscribe: {}", err);
                    }
                    return Ok(());
                }
            },
            // subscription failed, reject the sink
            Err(e) => {
                pending_sink.reject(errors::map_error(e)).await;
                return Ok(());
            }
        };

        loop {
            tokio::select! {
                msg = subscription.next() => {
                    match msg {
                        Some(resp) => {
                            let resp = match resp {
                                Ok(resp) => resp,
                                Err(e) => {
                                    tracing::error!("Subscription error: {}", e);
                                    continue;
                                }
                            };
                            let resp = match SubscriptionMessage::from_json(&resp) {
                                Ok(resp) => resp,
                                Err(e) => {
                                    tracing::error!("Failed to serialize subscription response: {}", e);
                                    continue;
                                }
                            };
                            if let Err(e) = sink.send(resp).await {
                                tracing::error!("Failed to send subscription response: {}", e);
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = sink.closed() => {
                    if let Err(err) = subscription.unsubscribe().await {
                        tracing::error!("Failed to unsubscribe: {}", err);
                    }
                    break
                },
            }
        }

        Ok(())
    }
}
