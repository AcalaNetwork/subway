use super::{CallRequest, CallResult, SubscriptionRequest, SubscriptionResult};
use crate::{
    config::{RpcMethod, RpcSubscription},
    middlewares::{Middleware, MiddlewareBuilder},
    utils::TypeRegistryRef,
};

/// Creates a middleware for the given RPC method.
///
/// # Arguments
///
/// * `name` - The name of the middleware to create.
/// * `method` - A reference to the `RpcMethod` you want to apply middleware.
/// * `extensions` - A reference to the `TypeRegistryRef` extensions.
///
/// # Returns
///
/// Returns an optional boxed middleware that can be applied to the RPC method.
pub async fn create_method_middleware(
    name: &str,
    method: &RpcMethod,
    extensions: &TypeRegistryRef,
) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
    use super::methods::*;

    match name {
        "response" => response::ResponseMiddleware::build(method, extensions).await,
        "upstream" => upstream::UpstreamMiddleware::build(method, extensions).await,
        "cache" => cache::CacheMiddleware::build(method, extensions).await,
        "block_tag" => block_tag::BlockTagMiddleware::build(method, extensions).await,
        "inject_params" => inject_params::InjectParamsMiddleware::build(method, extensions).await,
        "delay" => delay::DelayMiddleware::build(method, extensions).await,
        "validate" => validate::ValidateMiddleware::build(method, extensions).await,
        #[cfg(test)]
        "crazy" => testing::CrazyMiddleware::build(method, extensions).await,
        _ => panic!("Unknown method middleware: {}", name),
    }
}

/// Creates a middleware for a given RPC subscription.
///
/// # Arguments
///
/// * `name` - The name of the middleware to create.
/// * `method` - A reference to the `RpcSubscription` you want to apply middleware.
/// * `extensions` - A reference to the `TypeRegistryRef` extensions.
///
/// # Returns
///
/// Returns an optional boxed middleware that can be applied to the RPC subscription.
pub async fn create_subscription_middleware(
    name: &str,
    method: &RpcSubscription,
    extensions: &TypeRegistryRef,
) -> Option<Box<dyn Middleware<SubscriptionRequest, SubscriptionResult>>> {
    use super::subscriptions::*;

    match name {
        "upstream" => upstream::UpstreamMiddleware::build(method, extensions).await,
        "merge_subscription" => merge_subscription::MergeSubscriptionMiddleware::build(method, extensions).await,
        _ => panic!("Unknown subscription middleware: {}", name),
    }
}
