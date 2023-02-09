use async_trait::async_trait;
use jsonrpsee::{
    core::SubscriptionCallbackError, types::Params, PendingSubscriptionSink, SubscriptionMessage,
};
use std::sync::Arc;

use super::{Middleware, NextFn};
use crate::client::Client;

pub struct SubscriptionRequest {
    pub subscribe: String,
    pub params: Params<'static>,
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
impl Middleware<SubscriptionRequest, Result<(), SubscriptionCallbackError>> for UpstreamMiddleware {
    async fn call(
        &self,
        request: SubscriptionRequest,
        _next: NextFn<SubscriptionRequest, Result<(), SubscriptionCallbackError>>,
    ) -> Result<(), SubscriptionCallbackError> {
        let sink = request.sink;

        let mut sub = self
            .client
            .subscribe(
                &request.subscribe,
                request.params.parse()?,
                &request.unsubscribe,
            )
            .await
            .map_err(|e| SubscriptionCallbackError::Some(e.to_string()))?;

        let sink = sink.accept().await?;

        while let Some(resp) = sub.next().await {
            let resp = match resp {
                Ok(resp) => resp,
                Err(e) => {
                    log::error!("Subscription error: {}", e);
                    continue;
                }
            };
            let resp = match SubscriptionMessage::from_json(&resp) {
                Ok(resp) => resp,
                Err(e) => {
                    log::error!("Failed to serialize subscription response: {}", e);
                    continue;
                }
            };
            if let Err(e) = sink.send(resp).await {
                log::debug!("Failed to send subscription response: {}", e);
                break;
            }
        }
        Ok(())
    }
}
