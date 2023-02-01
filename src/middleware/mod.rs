use std::sync::Arc;

use async_trait::async_trait;
use futures::{future::BoxFuture, FutureExt};
use jsonrpsee::core::{Error, JsonValue};
use jsonrpsee::types::Params;
use jsonrpsee::SubscriptionSink;

pub mod cache;
pub mod upstream;

pub enum Request {
    Call {
        method: String,
        params: Params<'static>,
    },
    Subscription {
        subscribe: String,
        params: Params<'static>,
        unsubscribe: String,
        sink: SubscriptionSink,
    },
}

type NextFn<Request, Result> = Box<dyn FnOnce(Request) -> BoxFuture<'static, Result> + Send + Sync>;

#[async_trait]
pub trait Middleware: Send + Sync {
    async fn call(
        &self,
        request: Request,
        next: NextFn<Request, Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error>;
}

pub struct Middlewares {
    middlewares: Vec<Arc<dyn Middleware>>,
    fallback: Arc<dyn Fn(Request) -> BoxFuture<'static, Result<JsonValue, Error>> + Send + Sync>,
}

impl Middlewares {
    pub fn new(
        middlewares: Vec<Arc<dyn Middleware>>,
        fallback: Arc<
            dyn Fn(Request) -> BoxFuture<'static, Result<JsonValue, Error>> + Send + Sync,
        >,
    ) -> Self {
        Self {
            middlewares,
            fallback,
        }
    }

    pub async fn call(&self, request: Request) -> Result<JsonValue, Error> {
        let iter = self.middlewares.iter().rev();
        let fallback = self.fallback.clone();
        let mut next: Box<
            dyn FnOnce(Request) -> BoxFuture<'static, Result<JsonValue, Error>> + Send + Sync,
        > = Box::new(move |request| (fallback)(request));

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
