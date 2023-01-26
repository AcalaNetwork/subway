use std::sync::{Arc, atomic::AtomicU32};

use async_trait::async_trait;
use jsonrpsee::{
    core::{Error, JsonValue},
    types::Params,
};

use crate::client::Client;

pub struct Request {
    pub method: String,
    pub params: Params<'static>,
}

#[async_trait]
pub trait Middleware {
    type Response;
    type Error;

    async fn call(&self, request: Request) -> Result<Self::Response, Self::Error>;
}

// TODO: figure out how to setup middleware for subscriptions

pub struct LogMiddleware<Next> {
    count: AtomicU32,
    next: Next,
}

impl<Next> LogMiddleware<Next> {
    pub fn new(next: Next) -> Self {
        Self { count: Default::default(), next }
    }
}

#[async_trait]
impl<Next> Middleware for LogMiddleware<Next>
where
    Next: Middleware<Response = JsonValue, Error = Error> + Send + Sync,
{
    type Response = JsonValue;
    type Error = Error;

    async fn call(&self, request: Request) -> Result<Self::Response, Self::Error> {
        self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let val = self.count.load(std::sync::atomic::Ordering::SeqCst);
        log::info!("> {}: {}", val, request.method);
        let res = self.next.call(request).await;
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
impl Middleware for UpstreamMiddleware {
    type Response = JsonValue;
    type Error = Error;

    async fn call(&self, request: Request) -> Result<Self::Response, Self::Error> {
        self.client.request(&request.method, request.params).await
    }
}
