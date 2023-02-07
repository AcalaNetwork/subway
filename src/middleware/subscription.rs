use async_trait::async_trait;
use jsonrpsee::{
    core::{client::Subscription, Error, JsonValue},
    types::Params,
};
use std::sync::Arc;

use super::{Middleware, NextFn};
use crate::client::Client;

pub struct SubscriptionRequest {
    pub subscribe: String,
    pub params: Params<'static>,
    pub unsubscribe: String,
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
impl Middleware<SubscriptionRequest, Result<Subscription<JsonValue>, Error>>
    for UpstreamMiddleware
{
    async fn call(
        &self,
        request: SubscriptionRequest,
        _next: NextFn<SubscriptionRequest, Result<Subscription<JsonValue>, Error>>,
    ) -> Result<Subscription<JsonValue>, Error> {
        self.client
            .subscribe(
                &request.subscribe,
                request.params.parse()?,
                &request.unsubscribe,
            )
            .await
    }
}
