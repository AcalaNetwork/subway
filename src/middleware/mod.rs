use std::sync::Arc;

use async_trait::async_trait;
use futures::{future::BoxFuture, FutureExt};

pub mod call;
pub mod subscription;

type NextFn<Request, Result> = Box<dyn FnOnce(Request) -> BoxFuture<'static, Result> + Send + Sync>;

#[async_trait]
pub trait Middleware<Request, Result: 'static>: Send + Sync {
    async fn call(&self, request: Request, next: NextFn<Request, Result>) -> Result;
}

pub struct Middlewares<Request, Result> {
    middlewares: Vec<Arc<dyn Middleware<Request, Result>>>,
    fallback: Arc<dyn Fn(Request) -> BoxFuture<'static, Result> + Send + Sync>,
}

impl<Request: Send + 'static, Result: 'static> Middlewares<Request, Result> {
    pub fn new(
        middlewares: Vec<Arc<dyn Middleware<Request, Result>>>,
        fallback: Arc<dyn Fn(Request) -> BoxFuture<'static, Result> + Send + Sync>,
    ) -> Self {
        Self {
            middlewares,
            fallback,
        }
    }

    pub async fn call(&self, request: Request) -> Result {
        let mut iter = self.middlewares.iter().rev();
        let fallback = self.fallback.clone();
        let mut next: Box<dyn FnOnce(Request) -> BoxFuture<'static, Result> + Send + Sync> =
            Box::new(move |request| (fallback)(request));

        while let Some(middleware) = iter.next() {
            let middleware = middleware.clone();
            let next2 = next;
            next = Box::new(move |request| {
                async move { middleware.call(request, next2).await }.boxed()
            });
        }

        (next)(request).await
    }
}
