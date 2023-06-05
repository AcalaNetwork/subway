use async_trait::async_trait;
use futures::{future::BoxFuture, FutureExt};
use std::sync::Arc;

pub trait Provider<T, P> {
    const NAME: &'static str;

    fn new() -> Self;

    fn provide(&self, params: &P) -> T;
}

type NextFn<Request, Result> = Box<dyn FnOnce(Request) -> BoxFuture<'static, Result> + Send + Sync>;

#[async_trait]
pub trait Middleware: Send + Sync {
    async fn call(&self, request: Request, next: NextFn<Request, Result>) -> Result;
}

pub struct Middlewares {
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
        let iter = self.middlewares.iter().rev();
        let fallback = self.fallback.clone();
        let mut next: Box<dyn FnOnce(Request) -> BoxFuture<'static, Result> + Send + Sync> =
            Box::new(move |request| (fallback)(request));

        for middleware in iter {
            let middleware = middleware.clone();
            let next2 = next;
            next = Box::new(move |request| {
                async move { middleware.call(request, next2).await }.boxed()
            });
        }

        (next)(request).await
    }
}
