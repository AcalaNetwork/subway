use async_trait::async_trait;
use jsonrpsee::{
    core::{Error, JsonValue},
    types::error::CallError,
};
use std::sync::Arc;

use super::{Middleware, NextFn};
use crate::{
    api::{Api, ValueHandle},
    config::MethodParam,
    middleware::call::CallRequest,
};

pub enum InjectType {
    BlockHashAt(usize),
    BlockNumberAt(usize),
}

pub struct InjectParamsMiddleware {
    head: ValueHandle<(JsonValue, u64)>,
    inject: InjectType,
    params: Vec<MethodParam>,
}

impl InjectParamsMiddleware {
    pub fn new(api: Arc<Api>, inject: InjectType, params: Vec<MethodParam>) -> Self {
        Self {
            head: api.get_head(),
            inject,
            params,
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

pub fn inject(params: &Vec<MethodParam>) -> Option<InjectType> {
    let maybe_block_num = params
        .iter()
        .position(|p| p.inject == Some(true) && p.ty == "BlockNumber");
    if let Some(block_num) = maybe_block_num {
        return Some(InjectType::BlockNumberAt(block_num));
    }

    let maybe_block_hash = params
        .iter()
        .position(|p| p.inject == Some(true) && p.ty == "BlockHash");
    if let Some(block_hash) = maybe_block_hash {
        return Some(InjectType::BlockHashAt(block_hash));
    }

    None
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
            len if len <= idx => {
                // without current block
                let to_inject = self.get_parameter().await;
                tracing::debug!("Injected param {} to method {}", &to_inject, request.method);
                while request.params.len() < idx {
                    let current = request.params.len();
                    if self.params[current].is_optional == Some(true) {
                        request.params.push(JsonValue::Null);
                    } else {
                        return Err(Error::Call(CallError::InvalidParams(anyhow::Error::msg(
                            "non-optional param",
                        ))));
                    }
                }
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
