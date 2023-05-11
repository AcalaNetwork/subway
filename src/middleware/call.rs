use std::sync::Arc;

use async_trait::async_trait;
use jsonrpsee::{core::JsonValue, types::ErrorObjectOwned};
use tracing::instrument;

use super::{Middleware, NextFn};
use crate::client::Client;

#[derive(Debug, Default)]
pub struct Extra {
    pub bypass_cache: bool,
}

#[derive(Debug)]
pub struct CallRequest {
    pub method: String,
    pub params: Vec<JsonValue>,
    pub extra: Extra,
}

impl CallRequest {
    pub fn new(method: impl AsRef<str>, params: Vec<JsonValue>) -> Self {
        Self {
            method: method.as_ref().to_string(),
            params,
            extra: Extra::default(),
        }
    }
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
impl Middleware<CallRequest, Result<JsonValue, ErrorObjectOwned>> for UpstreamMiddleware {
    #[instrument(skip_all)]
    async fn call(
        &self,
        request: CallRequest,
        _next: NextFn<CallRequest, Result<JsonValue, ErrorObjectOwned>>,
    ) -> Result<JsonValue, ErrorObjectOwned> {
        self.client.request(&request.method, request.params).await
    }
}
