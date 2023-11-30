use futures::{future::BoxFuture, FutureExt};
use governor::{DefaultKeyedRateLimiter, Jitter};
use jsonrpsee::{
    server::{middleware::rpc::RpcServiceT, types::Request},
    MethodResponse,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct IpRateLimitLayer {
    ip_addr: String,
    limiter: Arc<DefaultKeyedRateLimiter<String>>,
    jitter: Jitter,
}

impl IpRateLimitLayer {
    pub fn new(key: String, limiter: Arc<DefaultKeyedRateLimiter<String>>, jitter: Jitter) -> Self {
        Self {
            ip_addr: key,
            limiter,
            jitter,
        }
    }
}

impl<S> tower::Layer<S> for IpRateLimitLayer {
    type Service = IpRateLimit<S>;

    fn layer(&self, service: S) -> Self::Service {
        IpRateLimit::new(service, self.ip_addr.clone(), self.limiter.clone(), self.jitter)
    }
}

#[derive(Clone)]
pub struct IpRateLimit<S> {
    service: S,
    ip_addr: String,
    limiter: Arc<DefaultKeyedRateLimiter<String>>,
    jitter: Jitter,
}

impl<S> IpRateLimit<S> {
    pub fn new(service: S, ip_addr: String, limiter: Arc<DefaultKeyedRateLimiter<String>>, jitter: Jitter) -> Self {
        Self {
            service,
            ip_addr,
            limiter,
            jitter,
        }
    }
}

impl<'a, S> RpcServiceT<'a> for IpRateLimit<S>
where
    S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
{
    type Future = BoxFuture<'a, MethodResponse>;

    fn call(&self, req: Request<'a>) -> Self::Future {
        let ip_addr = self.ip_addr.clone();
        let jitter = self.jitter;
        let service = self.service.clone();
        let limiter = self.limiter.clone();

        async move {
            limiter.until_key_ready_with_jitter(&ip_addr, jitter).await;
            service.call(req).await
        }
        .boxed()
    }
}
