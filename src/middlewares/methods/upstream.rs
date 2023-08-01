use std::sync::Arc;

use async_trait::async_trait;
use jsonrpsee::{core::JsonValue, types::ErrorObjectOwned};

use crate::{
    extensions::client::Client,
    middleware::{Middleware, MiddlewareBuilder, NextFn, RpcMethod},
    middlewares::{CallRequest, CallResult},
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
impl MiddlewareBuilder<CallRequest, CallResult> for UpstreamMiddleware {
    async fn build(
        _method: &RpcMethod,
        extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
        let client = extensions
            .read()
            .await
            .get::<Client>()
            .expect("Client extension not found");
        Some(Box::new(UpstreamMiddleware::new(client)))
    }
}

#[async_trait]
impl Middleware<CallRequest, Result<JsonValue, ErrorObjectOwned>> for UpstreamMiddleware {
    async fn call(
        &self,
        request: CallRequest,
        _context: TypeRegistry,
        _next: NextFn<CallRequest, Result<JsonValue, ErrorObjectOwned>>,
    ) -> Result<JsonValue, ErrorObjectOwned> {
        self.client.request(&request.method, request.params).await
    }
}
