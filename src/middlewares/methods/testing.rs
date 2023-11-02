#![cfg(test)]

use async_trait::async_trait;

use crate::{
    middleware::{Middleware, MiddlewareBuilder, NextFn, RpcMethod},
    middlewares::{CallRequest, CallResult},
    utils::{TypeRegistry, TypeRegistryRef},
};

pub struct CrazyMiddleware(bool);

impl CrazyMiddleware {
    pub fn new(go_crazy: bool) -> Self {
        Self(go_crazy)
    }
}

#[async_trait]
impl MiddlewareBuilder<RpcMethod, CallRequest, CallResult> for CrazyMiddleware {
    async fn build(
        method: &RpcMethod,
        _extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
        Some(Box::new(CrazyMiddleware::new(method.method == "go_crazy")))
    }
}

#[async_trait]
impl Middleware<CallRequest, CallResult> for CrazyMiddleware {
    async fn call(
        &self,
        request: CallRequest,
        context: TypeRegistry,
        next: NextFn<CallRequest, CallResult>,
    ) -> CallResult {
        if self.0 {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_nanos(1)).await
            }
        } else {
            next(request, context).await
        }
    }
}
