use async_trait::async_trait;
use jsonrpsee::core::{Error, JsonValue};
use std::sync::Arc;

use super::{Middleware, NextFn};
use crate::{
    api::{Api, ValueHandle},
    middleware::call::CallRequest,
};

pub enum InjectType {
    BlockHashAt(usize),
    BlockNumberAt(usize),
}

pub struct InjectParamsMiddleware {
    head: ValueHandle<(JsonValue, u64)>,
    inject: InjectType,
}

impl InjectParamsMiddleware {
    pub fn new(api: Arc<Api>, inject: InjectType) -> Self {
        Self {
            head: api.get_head(),
            inject,
        }
    }

    fn get_index(&self) -> usize {
        match self.inject {
            InjectType::BlockHashAt(index) => index,
            InjectType::BlockNumberAt(index) => index,
        }
    }

    async fn get_parameter(&self) -> JsonValue {
        let res = self.head.read().await;
        match self.inject {
            InjectType::BlockHashAt(_) => res.0,
            InjectType::BlockNumberAt(_) => res.1.into(),
        }
    }
}

#[async_trait]
impl Middleware<CallRequest, Result<JsonValue, Error>> for InjectParamsMiddleware {
    async fn call(
        &self,
        mut request: CallRequest,
        next: NextFn<CallRequest, Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        let idx = self.get_index();
        match request.params.len() {
            len if len == idx + 1 => {
                // full params with current block
                return next(request).await;
            }
            len if len == idx => {
                // without current block
                let to_inject = self.get_parameter().await;
                tracing::debug!("Injected param {} to method {}", &to_inject, request.method);
                request.params.push(to_inject);
                return next(request).await;
            }
            _ => {
                // unexpected number of params
                next(request).await
            }
        }
    }
}
