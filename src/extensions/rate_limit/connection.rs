use futures::{future::BoxFuture, FutureExt};
use governor::{DefaultDirectRateLimiter, Jitter, Quota, RateLimiter};
use jsonrpsee::{
    server::{middleware::rpc::RpcServiceT, types::Request},
    MethodResponse,
};
use std::num::NonZeroU32;
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct ConnectionRateLimitLayer {
    burst: NonZeroU32,
    period: Duration,
    jitter: Jitter,
}

impl ConnectionRateLimitLayer {
    pub fn new(burst: NonZeroU32, period: Duration, jitter: Jitter) -> Self {
        Self { burst, period, jitter }
    }
}

impl<S> tower::Layer<S> for ConnectionRateLimitLayer {
    type Service = ConnectionRateLimit<S>;

    fn layer(&self, service: S) -> Self::Service {
        ConnectionRateLimit::new(service, self.burst, self.period, self.jitter)
    }
}

#[derive(Clone)]
pub struct ConnectionRateLimit<S> {
    service: S,
    limiter: Arc<DefaultDirectRateLimiter>,
    jitter: Jitter,
}

impl<S> ConnectionRateLimit<S> {
    pub fn new(service: S, burst: NonZeroU32, period: Duration, jitter: Jitter) -> Self {
        let replenish_interval_ns = period.as_nanos() / (burst.get() as u128);
        let quota = Quota::with_period(Duration::from_nanos(replenish_interval_ns as u64))
            .unwrap()
            .allow_burst(burst);
        let limiter = Arc::new(RateLimiter::direct(quota));
        Self {
            service,
            limiter,
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
            NonZeroU32::new(10).unwrap(),
            Duration::from_millis(100),
            Jitter::up_to(Duration::from_millis(10)),
        );

        let batch = |service: ConnectionRateLimit<MockService>, count: usize, delay| async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            let calls = (1..=count)
                .map(|id| service.call(Request::new("test".into(), None, Id::Number(id as u64))))
                .collect::<Vec<_>>();
            let results = futures::future::join_all(calls).await;
            assert_eq!(results.iter().filter(|r| r.is_success()).count(), count);
        };

        let start = tokio::time::Instant::now();
        // background task to make calls
        let batch1 = tokio::spawn(batch(service.clone(), 30, 0));
        let batch2 = tokio::spawn(batch(service.clone(), 40, 200));
        let batch3 = tokio::spawn(batch(service.clone(), 20, 300));
        batch1.await.unwrap();
        batch2.await.unwrap();
        batch3.await.unwrap();
        let duration = start.elapsed().as_millis();
        println!("duration: {} ms", duration);
        // should take between 800..900 millis. each 100ms period handles 10 calls
        assert!(duration > 800 && duration < 900);
    }
}
