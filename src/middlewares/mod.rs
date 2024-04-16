use async_trait::async_trait;
use futures::{future::BoxFuture, FutureExt};
use jsonrpsee::{
    core::{JsonValue, StringError},
    types::ErrorObjectOwned,
    PendingSubscriptionSink,
};
use opentelemetry::trace::FutureExt as _;
use serde::Serialize;
use std::{
    fmt::{Debug, Formatter},
    sync::Arc,
};

use crate::{
    config::{RpcMethod, RpcSubscription},
    utils::{errors, telemetry, TypeRegistry, TypeRegistryRef},
};

pub mod factory;
pub mod methods;
pub mod subscriptions;

#[derive(Clone, Debug, Serialize)]
/// Represents a RPC request made to a middleware function.
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

/// Alias for the result of a method request.
pub type CallResult = Result<JsonValue, ErrorObjectOwned>;

/// Represents a RPC subscription made to a middleware function.
pub struct SubscriptionRequest {
    pub subscribe: String,
    pub params: Vec<JsonValue>,
    pub unsubscribe: String,
    pub pending_sink: PendingSubscriptionSink,
}

impl Debug for SubscriptionRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscriptionRequest")
            .field("subscribe", &self.subscribe)
            .field("params", &self.params)
            .field("unsubscribe", &self.unsubscribe)
            .field("sink_id", &self.pending_sink.connection_id())
            .finish()
    }
}

/// Alias for the result of a subscription request.
pub type SubscriptionResult = Result<(), StringError>;

/// A trait for defining middleware functions that can be used to intercept and modify requests and responses.
#[async_trait]
pub trait Middleware<Request, Result>: Send + Sync {
    /// The method that will be called to handle the request.
    async fn call(&self, request: Request, context: TypeRegistry, next: NextFn<Request, Result>) -> Result;
}

/// A type alias for the next function to be called in the middleware chain.
pub type NextFn<Request, Result> = Box<dyn FnOnce(Request, TypeRegistry) -> BoxFuture<'static, Result> + Send + Sync>;

/// A trait for defining middleware builders that can be used to create middleware instances.
#[async_trait]
pub trait MiddlewareBuilder<Method, Request, Result> {
    /// The method that will be called to build the middleware instance.
    async fn build(method: &Method, extensions: &TypeRegistryRef) -> Option<Box<dyn Middleware<Request, Result>>>;
}

/// A struct that holds a list of middlewares and a fallback function to be called if no middleware handles the request.
pub struct Middlewares<Request, Result> {
    middlewares: Vec<Arc<dyn Middleware<Request, Result>>>,
    fallback: Arc<dyn Fn(Request, TypeRegistry) -> BoxFuture<'static, Result> + Send + Sync>,
}

impl<Request, Result> Clone for Middlewares<Request, Result> {
    fn clone(&self) -> Self {
        Self {
            middlewares: self.middlewares.clone(),
            fallback: self.fallback.clone(),
        }
    }
}

const TRACER: telemetry::Tracer = telemetry::Tracer::new("middlewares");

impl<Request: Debug + Send + 'static, Result: Send + 'static> Middlewares<Request, Result> {
    /// Creates a new middleware instance with the given middlewares and fallback function.
    ///
    /// # Arguments
    ///
    /// * `middlewares` - A vector of middlewares to be executed in order.
    /// * `fallback` - A function that will be called if no middleware handles the request.
    pub fn new(
        middlewares: Vec<Arc<dyn Middleware<Request, Result>>>,
        fallback: Arc<dyn Fn(Request, TypeRegistry) -> BoxFuture<'static, Result> + Send + Sync>,
    ) -> Self {
        Self { middlewares, fallback }
    }

    /// Calls the middleware chain with the given request and result sender.
    ///
    /// # Arguments
    ///
    /// * `request` - A `Request` struct representing the incoming request.
    /// * `result_tx` - A `tokio::sync::oneshot::Sender<Result>` used to send the result of the middleware chain.
    /// * `timeout` - A `tokio::time::Duration` representing the maximum time allowed for the middleware chain to complete.
    ///
    pub async fn call(
        &self,
        request: Request,
        result_tx: tokio::sync::oneshot::Sender<Result>,
        timeout: tokio::time::Duration,
    ) {
        let iter = self.middlewares.iter().rev();
        let fallback = self.fallback.clone();
        let mut next: Box<dyn FnOnce(Request, TypeRegistry) -> BoxFuture<'static, Result> + Send + Sync> =
            Box::new(move |request, context| (fallback)(request, context));

        for middleware in iter {
            let middleware = middleware.clone();
            let next2 = next;
            next =
                Box::new(move |request, context| async move { middleware.call(request, context, next2).await }.boxed());
        }

        let req = format!("{:?}", request);

        let mut task_handle = tokio::spawn(
            async move {
                let result = next(request, TypeRegistry::new()).await;
                _ = result_tx.send(result);

                opentelemetry::trace::get_active_span(|span| {
                    span.set_status(opentelemetry::trace::Status::Ok);
                });
            }
            .with_context(TRACER.context("middlewares")),
        );

        let sleep = tokio::time::sleep(timeout);

        tokio::select! {
            _ = sleep => {
                tracing::error!("middlewares timeout: {req}");
                TRACER.span_error(&errors::failed("middlewares timeout"));
                task_handle.abort();
            }
            _ = &mut task_handle => {
                tracing::trace!("middlewares finished: {req}");
            }
        }
    }
}
