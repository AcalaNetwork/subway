use async_trait::async_trait;
use futures::{future::BoxFuture, FutureExt};
use std::sync::Arc;

use crate::utils::TypeRegistryRef;
pub use crate::{
    config::{RpcMethod, RpcSubscription},
    utils::TypeRegistry,
};

#[async_trait]
pub trait MiddlewareBuilder<Method, Request, Result> {
    async fn build(
        method: &Method,
        extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<Request, Result>>>;
}

pub type NextFn<Request, Result> =
    Box<dyn FnOnce(Request, TypeRegistry) -> BoxFuture<'static, Result> + Send + Sync>;

#[async_trait]
pub trait Middleware<Request, Result>: Send + Sync {
    async fn call(
        &self,
        request: Request,
        context: TypeRegistry,
        next: NextFn<Request, Result>,
    ) -> Result;
}

pub struct Middlewares<Request, Result> {
    middlewares: Vec<Arc<dyn Middleware<Request, Result>>>,
    fallback: Arc<dyn Fn(Request, TypeRegistry) -> BoxFuture<'static, Result> + Send + Sync>,
}

impl<Request: Send + 'static, Result: 'static> Middlewares<Request, Result> {
    pub fn new(
        middlewares: Vec<Arc<dyn Middleware<Request, Result>>>,
        fallback: Arc<dyn Fn(Request, TypeRegistry) -> BoxFuture<'static, Result> + Send + Sync>,
    ) -> Self {
        Self {
            middlewares,
            fallback,
        }
    }

    pub async fn call(&self, request: Request) -> Result {
        let iter = self.middlewares.iter().rev();
        let fallback = self.fallback.clone();
        let mut next: Box<
            dyn FnOnce(Request, TypeRegistry) -> BoxFuture<'static, Result> + Send + Sync,
        > = Box::new(move |request, context| (fallback)(request, context));

        for middleware in iter {
            let middleware = middleware.clone();
            let next2 = next;
            next = Box::new(move |request, context| {
                async move { middleware.call(request, context, next2).await }.boxed()
            });
        }

        (next)(request, TypeRegistry::new()).await
    }
}
