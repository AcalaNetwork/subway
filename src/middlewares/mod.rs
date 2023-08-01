use jsonrpsee::{core::JsonValue, types::ErrorObjectOwned};

use crate::{
    config::RpcMethod,
    middleware::{Middleware, MiddlewareBuilder},
    utils::TypeRegistryRef,
};

pub mod methods;
// pub mod subscriptions;

#[derive(Debug)]
pub struct CallRequest {
    pub method: String,
    pub params: Vec<JsonValue>,
}

impl CallRequest {
    pub fn new(method: impl ToString, params: Vec<JsonValue>) -> Self {
        Self {
            method: method.to_string(),
            params,
        }
    }
}

pub type CallResult = Result<JsonValue, ErrorObjectOwned>;

pub async fn create_method_middleware(
    name: &str,
    method: &RpcMethod,
    extensions: &TypeRegistryRef,
) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
    use methods::*;

    match name {
        "response" => response::ResponseMiddleware::build(method, extensions).await,
        "upstream" => upstream::UpstreamMiddleware::build(method, extensions).await,
        "cache" => cache::CacheMiddleware::build(method, extensions).await,
        "block_tag" => block_tag::BlockTagMiddleware::build(method, extensions).await,
        "inject_params" => inject_params::InjectParamsMiddleware::build(method, extensions).await,
        _ => None,
    }
}
