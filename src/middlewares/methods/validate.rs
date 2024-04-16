use async_trait::async_trait;
use std::sync::Arc;

use crate::{
    extensions::{client::Client, validator::Validator},
    middlewares::{CallRequest, CallResult, Middleware, MiddlewareBuilder, NextFn, RpcMethod},
    utils::{TypeRegistry, TypeRegistryRef},
};

pub struct ValidateMiddleware {
    validator: Arc<Validator>,
    client: Arc<Client>,
}

impl ValidateMiddleware {
    pub fn new(validator: Arc<Validator>, client: Arc<Client>) -> Self {
        Self { validator, client }
    }
}

#[async_trait]
impl MiddlewareBuilder<RpcMethod, CallRequest, CallResult> for ValidateMiddleware {
    async fn build(
        _method: &RpcMethod,
        extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
        let validate = extensions.read().await.get::<Validator>().unwrap_or_default();

        let client = extensions
            .read()
            .await
            .get::<Client>()
            .expect("Client extension not found");
        Some(Box::new(ValidateMiddleware::new(validate, client)))
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
            self.validator.validate(self.client.clone(), request, result.clone());
        }
        result
    }
}
