use crate::client::Client;
use async_trait::async_trait;
use jsonrpsee::core::Error;
use jsonrpsee::core::JsonValue;
use std::sync::Arc;

use super::{Middleware, NextFn, Request};

pub struct SubscriptionMiddleware {
    client: Arc<Client>,
}

impl SubscriptionMiddleware {
    pub fn new(client: &Arc<Client>) -> Self {
        Self {
            client: client.clone(),
        }
    }
}

#[async_trait]
impl Middleware for SubscriptionMiddleware {
    async fn call(
        &self,
        request: Request,
        next: NextFn<Request, Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        match request {
            Request::Subscription {
                subscribe,
                params,
                unsubscribe,
                mut sink,
            } => {
                let sub = self
                    .client
                    .subscribe(&subscribe, params.parse()?, &unsubscribe)
                    .await?;
                sink.pipe_from_try_stream(sub).await;
                Ok(().into())
            }
            _ => next(request).await,
        }
    }
}
