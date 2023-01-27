use std::sync::Arc;

use async_trait::async_trait;
use jsonrpsee::{
    core::{Error, JsonValue},
    types::Params,
};

use crate::client::Client;

use super::{Middleware, NextFn};

pub struct CallRequest {
    pub method: String,
    pub params: Params<'static>,
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
impl Middleware<CallRequest, Result<JsonValue, Error>> for UpstreamMiddleware {
    async fn call(
        &self,
        request: CallRequest,
        _next: NextFn<CallRequest, Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        self.client.request(&request.method, request.params).await
    }
}
