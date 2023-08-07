use std::sync::Arc;

use async_trait::async_trait;
use jsonrpsee::SubscriptionMessage;

use crate::{
    extensions::client::Client,
    middleware::{Middleware, MiddlewareBuilder, NextFn, RpcSubscription},
    middlewares::{SubscriptionRequest, SubscriptionResult},
    utils::{TypeRegistry, TypeRegistryRef},
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
impl MiddlewareBuilder<RpcSubscription, SubscriptionRequest, SubscriptionResult>
    for UpstreamMiddleware
{
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
        let sink = request.sink;

        let mut sub = self
            .client
            .subscribe(&request.subscribe, request.params, &request.unsubscribe)
            .await?;

        let sink = sink.accept().await?;

        while let Some(resp) = sub.next().await {
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
                tracing::info!("Failed to send subscription response: {}", e);
                break;
            }
        }
        Ok(())
    }
}
