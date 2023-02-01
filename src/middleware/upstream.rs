use crate::client::Client;
use async_trait::async_trait;
use jsonrpsee::{
    core::{error::SubscriptionClosed, Error, JsonValue},
    types::ErrorObjectOwned,
};
use std::sync::Arc;

use super::{Middleware, NextFn, Request};

pub struct UpstreamMiddleware {
    client: Arc<Client>,
}

impl UpstreamMiddleware {
    pub fn new(client: &Arc<Client>) -> Self {
        Self {
            client: client.clone(),
        }
    }
}

#[async_trait]
impl Middleware for UpstreamMiddleware {
    async fn call(
        &self,
        request: Request,
        _next: NextFn<Request, Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        match request {
            Request::Call { method, params } => self.client.request(&method, params.parse()?).await,
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
                match sink.pipe_from_try_stream(sub).await {
                    SubscriptionClosed::Success => {
                        let err_obj: ErrorObjectOwned = SubscriptionClosed::Success.into();
                        sink.close(err_obj);
                    }
                    // we don't want to send close reason when the client is unsubscribed or disconnected.
                    SubscriptionClosed::RemotePeerAborted => (),
                    SubscriptionClosed::Failed(e) => {
                        sink.close(e);
                    }
                }
                Ok(().into())
            }
        }
    }
}
