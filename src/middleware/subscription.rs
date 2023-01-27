use std::sync::Arc;

use async_trait::async_trait;
use jsonrpsee::{core::Error, types::Params, SubscriptionSink};

use crate::client::Client;

use super::{Middleware, NextFn};

pub struct SubscriptionRequest {
    pub subscribe: String,
    pub params: Params<'static>,
    pub unsubscribe: String,
    pub sink: SubscriptionSink,
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
impl Middleware<SubscriptionRequest, Result<(), Error>> for UpstreamMiddleware {
    async fn call(
        &self,
        mut request: SubscriptionRequest,
        _next: NextFn<SubscriptionRequest, Result<(), Error>>,
    ) -> Result<(), Error> {
        let sub = self
            .client
            .subscribe(&request.subscribe, request.params, &request.unsubscribe)
            .await?;
        request.sink.pipe_from_try_stream(sub).await;
        Ok(())
    }
}
