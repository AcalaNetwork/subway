use async_trait::async_trait;
use std::sync::Arc;

use crate::{
    extensions::validator::Validator,
    middlewares::{CallRequest, CallResult, Middleware, MiddlewareBuilder, NextFn, RpcMethod},
    utils::{TypeRegistry, TypeRegistryRef},
};

pub struct ValidateMiddleware {
    validator: Arc<Validator>,
}

impl ValidateMiddleware {
    pub fn new(validator: Arc<Validator>) -> Self {
        Self { validator }
    }
}

#[async_trait]
impl MiddlewareBuilder<RpcMethod, CallRequest, CallResult> for ValidateMiddleware {
    async fn build(
        _method: &RpcMethod,
        extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
        let validate = extensions
            .read()
            .await
            .get::<Validator>()
            .expect("Validator extension not found");

        Some(Box::new(ValidateMiddleware::new(validate)))
    }
}

#[async_trait]
impl Middleware<CallRequest, CallResult> for ValidateMiddleware {
    async fn call(
        &self,
        request: CallRequest,
        context: TypeRegistry,
        next: NextFn<CallRequest, CallResult>,
    ) -> CallResult {
        let result = next(request.clone(), context).await;
        if !self.validator.ignore(&request.method) {
            self.validator.validate(request, result.clone());
        }
        result
    }
}
