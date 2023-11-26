use crate::extensions::server::{Period, RateLimitConfig};
use futures::{future::BoxFuture, FutureExt};
use governor::{DefaultDirectRateLimiter, Jitter, Quota, RateLimiter};
use jsonrpsee::{
    server::{middleware::rpc::RpcServiceT, types::Request},
    MethodResponse,
};
use std::num::NonZeroU32;
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct RateLimit {
    config: RateLimitConfig,
}

impl RateLimit {
    pub fn new(config: RateLimitConfig) -> Self {
        assert!(config.burst > 0, "burst must be greater than 0");
        Self { config }
    }
}

impl<S> tower::Layer<S> for RateLimit {
    type Service = ConnectionRateLimit<S>;

    fn layer(&self, service: S) -> Self::Service {
        let burst = NonZeroU32::new(self.config.burst).unwrap();
        let period = self.config.period;
        let jitter = Jitter::up_to(Duration::from_millis(self.config.jitter_millis));
        ConnectionRateLimit::new(service, burst, period, jitter)
    }
}

pub struct ConnectionRateLimit<S> {
    service: S,
    limiter: Arc<DefaultDirectRateLimiter>,
    jitter: Jitter,
}

impl<S> ConnectionRateLimit<S> {
    pub fn new(service: S, burst: NonZeroU32, period: Period, jitter: Jitter) -> Self {
        log::warn!("rate limiting is enabled");
        let quota = match period {
            Period::Second => Quota::per_second(burst),
            Period::Minute => Quota::per_minute(burst),
            Period::Hour => Quota::per_hour(burst),
        };
        Self {
            service,
            limiter: Arc::new(RateLimiter::direct(quota)),
            jitter,
        }
    }
}

impl<'a, S> RpcServiceT<'a> for ConnectionRateLimit<S>
where
    S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
{
    type Future = BoxFuture<'a, MethodResponse>;

    fn call(&self, req: Request<'a>) -> Self::Future {
        let jitter = self.jitter;
        let service = self.service.clone();
        let limiter = self.limiter.clone();

        async move {
            limiter.until_ready_with_jitter(jitter).await;
            service.call(req).await
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::types::{Id, ResponsePayload};

    #[derive(Clone)]
    struct MockService;
    impl RpcServiceT<'static> for MockService {
        type Future = BoxFuture<'static, MethodResponse>;

        fn call(&self, req: Request<'static>) -> Self::Future {
            async move { MethodResponse::response(req.id, ResponsePayload::result("ok"), 1024) }.boxed()
        }
    }

    #[tokio::test]
    async fn rate_limit_works() {
        let service = ConnectionRateLimit::new(
            MockService,
            NonZeroU32::new(20).unwrap(),
            Period::Second,
            Jitter::up_to(Duration::from_millis(100)),
        );

        let count = 60;
        let start = tokio::time::Instant::now();
        let calls = (1..=count)
            .map(|id| service.call(Request::new("test".into(), None, Id::Number(id))))
            .collect::<Vec<_>>();

        let results = futures::future::join_all(calls).await;
        let duration = start.elapsed().as_secs_f64();
        // should take at least 2 seconds
        assert!(duration > 2.0);
        // calls should succeed
        assert_eq!(results.iter().filter(|r| r.is_success()).count(), count as usize);
    }
}
