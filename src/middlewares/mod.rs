use jsonrpsee::{core::JsonValue, types::ErrorObjectOwned};

use crate::{
    config::RpcMethod,
    extensions,
    middleware::{Middleware, MiddlewareBuilder, NextFn},
    utils::{TypeRegistry, TypeRegistryRef},
};

pub mod methods;
// pub mod subscriptions;

#[derive(Debug)]
pub struct CallRequest {
    pub method: String,
    pub params: Vec<JsonValue>,
}

impl CallRequest {
    pub fn new(method: String, params: Vec<JsonValue>) -> Self {
        Self { method, params }
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
        _ => None,
    }
}
