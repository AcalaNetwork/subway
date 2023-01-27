use std::sync::{atomic::AtomicU32, Arc};

use async_trait::async_trait;
use futures::{future::BoxFuture, FutureExt};
use jsonrpsee::{
    core::{Error, JsonValue},
    types::Params,
};

use crate::client::Client;

pub struct Request {
    pub method: String,
    pub params: Params<'static>,
}

type NextFn<Result> = Box<dyn FnOnce(Request) -> BoxFuture<'static, Result> + Send + Sync>;

#[async_trait]
pub trait Middleware<Result: 'static>: Send + Sync {
    async fn call(&self, request: Request, next: NextFn<Result>) -> Result;
}

pub struct Middlewares<Result> {
    middlewares: Vec<Arc<dyn Middleware<Result>>>,
    fallback: Arc<dyn Fn(Request) -> BoxFuture<'static, Result> + Send + Sync>,
}

impl<Result: 'static> Middlewares<Result> {
    pub fn new(
        middlewares: Vec<Arc<dyn Middleware<Result>>>,
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
            let f = move |request| async move { middleware.call(request, next2).await }.boxed();
            next = Box::new(f);
        }

        (next)(request).await
    }
}

pub struct LogMiddleware {
    count: AtomicU32,
}

impl LogMiddleware {
    pub fn new() -> Self {
        Self {
            count: Default::default(),
        }
    }
}

#[async_trait]
impl Middleware<Result<JsonValue, Error>> for LogMiddleware {
    async fn call(
        &self,
        request: Request,
        next: NextFn<Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let val = self.count.load(std::sync::atomic::Ordering::SeqCst);
        log::info!("> {}: {}", val, request.method);
        let res = next(request).await;
        log::info!("< {}: {:?}", val, res);
        res
    }
}

pub struct UpstreamMiddleware {
    client: Arc<Client>,
}

impl UpstreamMiddleware {
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Middleware<Result<JsonValue, Error>> for UpstreamMiddleware {
    async fn call(
        &self,
        request: Request,
        _next: NextFn<Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        self.client.request(&request.method, request.params).await
    }
}
