use async_trait::async_trait;
use jsonrpsee::{core::JsonValue, types::ErrorObjectOwned};

use crate::middleware::{call::CallRequest, Middleware, NextFn};

pub struct ResponseMiddleware {
    resp: JsonValue,
}

impl ResponseMiddleware {
    pub fn new(resp: JsonValue) -> Self {
        Self { resp }
    }
}

#[async_trait]
impl Middleware<CallRequest, Result<JsonValue, ErrorObjectOwned>> for ResponseMiddleware {
    async fn call(
        &self,
        _request: CallRequest,
        _next: NextFn<CallRequest, Result<JsonValue, ErrorObjectOwned>>,
    ) -> Result<JsonValue, ErrorObjectOwned> {
        Ok(self.resp.clone())
    }
}
