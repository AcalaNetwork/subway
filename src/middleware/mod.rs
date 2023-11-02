use async_trait::async_trait;
use futures::{future::BoxFuture, FutureExt};
use std::{fmt::Debug, sync::Arc};

pub use crate::{
    config::{RpcMethod, RpcSubscription},
    extension::ExtensionRegistry,
    utils::{TypeRegistry, TypeRegistryRef},
};

#[async_trait]
pub trait MiddlewareBuilder<Method, Request, Result> {
    async fn build(method: &Method, extensions: &TypeRegistryRef) -> Option<Box<dyn Middleware<Request, Result>>>;
}

pub type NextFn<Request, Result> = Box<dyn FnOnce(Request, TypeRegistry) -> BoxFuture<'static, Result> + Send + Sync>;

#[async_trait]
pub trait Middleware<Request, Result>: Send + Sync {
    async fn call(&self, request: Request, context: TypeRegistry, next: NextFn<Request, Result>) -> Result;
}

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

impl<Request: Debug + Send + 'static, Result: Send + 'static> Middlewares<Request, Result> {
    pub fn new(
        middlewares: Vec<Arc<dyn Middleware<Request, Result>>>,
        fallback: Arc<dyn Fn(Request, TypeRegistry) -> BoxFuture<'static, Result> + Send + Sync>,
    ) -> Self {
        Self { middlewares, fallback }
    }

    pub async fn call(
        &self,
        request: Request,
        rt: Arc<tokio::runtime::Runtime>,
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

        let mut task_handle = rt.spawn(async move {
            let result = next(request, TypeRegistry::new()).await;
            _ = result_tx.send(result);
        });

        let sleep = tokio::time::sleep(timeout);

        tokio::select! {
            _ = sleep => {
                tracing::error!("middlewares timeout: {req}");
                task_handle.abort();
            }
            _ = &mut task_handle => {
                tracing::trace!("middlewares finished: {req}");
            }
        }
    }
}
