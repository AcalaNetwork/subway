use async_trait::async_trait;
use jsonrpsee::{
    core::{JsonValue, StringError},
    PendingSubscriptionSink, SubscriptionMessage,
};
use std::sync::Arc;

use crate::{
    client::Client,
    middleware::{Middleware, NextFn},
};

pub struct SubscriptionRequest {
    pub subscribe: String,
    pub params: Vec<JsonValue>,
    pub unsubscribe: String,
    pub sink: PendingSubscriptionSink,
}

pub struct UpstreamMiddleware {
    client: Arc<Client>,
}

impl UpstreamMiddleware {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Middleware<SubscriptionRequest, Result<(), StringError>> for UpstreamMiddleware {
    async fn call(
        &self,
        request: SubscriptionRequest,
        _next: NextFn<SubscriptionRequest, Result<(), StringError>>,
    ) -> Result<(), StringError> {
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
                tracing::debug!("Failed to send subscription response: {}", e);
                break;
            }
        }
        Ok(())
    }
}
