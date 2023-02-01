use crate::client::Client;
use async_trait::async_trait;
use jsonrpsee::core::{Error, JsonValue};
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
                sink.pipe_from_try_stream(sub).await;
                Ok(().into())
            }
        }
    }
}
