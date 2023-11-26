use crate::extensions::server::{Period, RateLimitConfig};
use futures::{future::BoxFuture, FutureExt};
use governor::{
    clock::DefaultClock,
    state::{InMemoryState, NotKeyed},
    Jitter, Quota, RateLimiter,
};
use jsonrpsee::{
    server::{middleware::rpc::RpcServiceT, types::Request},
    MethodResponse,
};
use std::num::NonZeroU32;
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct RateLimit<S> {
    service: S,
    limiter: Option<Arc<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>>,
    jitter: Jitter,
}

impl<S> RateLimit<S> {
    pub fn new(service: S, config: RateLimitConfig) -> Self {
        if config.burst == 0 {
            return Self {
                service,
                limiter: None,
                jitter: Jitter::up_to(Duration::default()),
            };
        }
        let burst = NonZeroU32::new(config.burst).unwrap();
        let quota = Some(match config.period {
            Period::Second => Quota::per_second(burst),
            Period::Minute => Quota::per_minute(burst),
            Period::Hour => Quota::per_hour(burst),
        });
        Self {
            service,
            limiter: quota.map(|q| Arc::new(RateLimiter::direct(q))),
            jitter: Jitter::up_to(Duration::from_millis(config.jitter_millis)),
        }
    }
}

impl<'a, S> RpcServiceT<'a> for RateLimit<S>
where
    S: RpcServiceT<'a> + Send + Sync + Clone + 'static,
{
    type Future = BoxFuture<'a, MethodResponse>;

    fn call(&self, req: Request<'a>) -> Self::Future {
        match self.limiter {
            Some(ref limiter) => {
                let jitter = self.jitter;
                let limiter = limiter.clone();
                let service = self.service.clone();
                async move {
                    limiter.until_ready_with_jitter(jitter).await;
                    service.call(req).await
                }
                .boxed()
            }
            None => self.service.call(req).boxed(),
        }
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
        let rate = RateLimitConfig {
            burst: 2,
            period: Period::Second,
            jitter_millis: 0,
        };
        let service = RateLimit::new(MockService, rate);

        let start = tokio::time::Instant::now();
        let calls = (1..=10u64)
            .into_iter()
            .map(|id| service.call(Request::new("test".into(), None, Id::Number(id))))
            .collect::<Vec<_>>();

        let results = futures::future::join_all(calls).await;
        // should take at least 4 seconds
        assert!(start.elapsed().as_secs_f64() > 4.0);
        // calls should succeed
        assert_eq!(results.iter().filter(|r| r.is_success()).count(), 10);
    }
}
