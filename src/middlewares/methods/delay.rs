use std::time::Duration;

use async_trait::async_trait;

use crate::{
    middlewares::{CallRequest, CallResult, Middleware, MiddlewareBuilder, NextFn, RpcMethod},
    utils::{TypeRegistry, TypeRegistryRef},
};

pub struct DelayMiddleware {
    delay: Duration,
}

impl DelayMiddleware {
    pub fn new(delay: u64) -> Self {
        Self {
            delay: Duration::from_millis(delay),
        }
    }
}

#[async_trait]
impl MiddlewareBuilder<RpcMethod, CallRequest, CallResult> for DelayMiddleware {
    async fn build(
        method: &RpcMethod,
        _extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
        method
            .delay_ms
            .map(|delay| Box::new(DelayMiddleware::new(delay)) as Box<dyn Middleware<CallRequest, CallResult>>)
    }
}

#[async_trait]
impl Middleware<CallRequest, CallResult> for DelayMiddleware {
    async fn call(
        &self,
        request: CallRequest,
        context: TypeRegistry,
        next: NextFn<CallRequest, CallResult>,
    ) -> CallResult {
        tokio::time::sleep(self.delay).await;
        next(request, context).await
    }
}
