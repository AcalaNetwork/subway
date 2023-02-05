use async_trait::async_trait;
use jsonrpsee::{
    core::{Error, JsonValue, __reexports::serde_json},
    types::Params,
};
use std::sync::Arc;

use super::{Middleware, NextFn};
use crate::{api::Api, middleware::call::CallRequest};

pub enum Inject {
    BlockHashAt(usize),
    BlockNumberAt(usize),
}

pub struct InjectParamsMiddleware {
    api: Arc<Api>,
    inject: Inject,
}

impl InjectParamsMiddleware {
    pub fn new(api: Arc<Api>, inject: Inject) -> Self {
        Self { api, inject }
    }

    async fn needs_to_inject(&self, params_len: usize) -> Option<JsonValue> {
        match self.inject {
            Inject::BlockHashAt(hash_index) => {
                if params_len == hash_index {
                    // block hash is missing
                    return self.api.get_block_hash().await;
                }
            }
            Inject::BlockNumberAt(number_index) => {
                if params_len == number_index {
                    // block number is missing
                    return self.api.get_block_number().await;
                }
            }
        }
        None
    }
}

#[async_trait]
impl Middleware<CallRequest, Result<JsonValue, Error>> for InjectParamsMiddleware {
    async fn call(
        &self,
        mut request: CallRequest,
        next: NextFn<CallRequest, Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        let params = request.params.parse::<JsonValue>()?;

        if let Some(params) = params.to_owned().as_array_mut() {
            if let Some(param) = self.needs_to_inject(params.len()).await {
                log::debug!("Injected param {} to method {}", &param, request.method);
                params.push(param);
                request.params =
                    Params::new(Some(serde_json::to_string(&params)?.as_str())).into_owned();
            }
        }

        next(request).await
    }
}
