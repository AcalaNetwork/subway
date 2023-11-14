use async_trait::async_trait;
use jsonrpsee::core::JsonValue;

use crate::{
    middlewares::{CallRequest, CallResult, Middleware, MiddlewareBuilder, NextFn, RpcMethod},
    utils::{TypeRegistry, TypeRegistryRef},
};

pub struct ResponseMiddleware {
    resp: JsonValue,
}

impl ResponseMiddleware {
    pub fn new(resp: JsonValue) -> Self {
        Self { resp }
    }
}

#[async_trait]
impl MiddlewareBuilder<RpcMethod, CallRequest, CallResult> for ResponseMiddleware {
    async fn build(
        method: &RpcMethod,
        _extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
        method
            .response
            .as_ref()
            .map(|resp| Box::new(ResponseMiddleware::new(resp.clone())) as Box<dyn Middleware<CallRequest, CallResult>>)
    }
}

#[async_trait]
impl Middleware<CallRequest, CallResult> for ResponseMiddleware {
    async fn call(
        &self,
        _request: CallRequest,
        _context: TypeRegistry,
        _next: NextFn<CallRequest, CallResult>,
    ) -> CallResult {
        Ok(self.resp.clone())
    }
}
