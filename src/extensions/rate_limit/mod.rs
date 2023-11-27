use futures::{future::BoxFuture, FutureExt};
use governor::{DefaultDirectRateLimiter, Jitter, Quota, RateLimiter};
use jsonrpsee::{
    server::{middleware::rpc::RpcServiceT, types::Request},
    MethodResponse,
};
use serde::Deserialize;
use std::num::NonZeroU32;
use std::{sync::Arc, time::Duration};

use super::{Extension, ExtensionRegistry};

#[derive(Deserialize, Default, Debug, Copy, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    #[default]
    Second,
    Minute,
    Hour,
}

#[derive(Deserialize, Debug, Copy, Clone, Default)]
pub struct RateLimitConfig {
    pub burst: u32,
    pub period: Period,
    #[serde(default = "default_jitter_millis")]
    pub jitter_millis: u64,
}

fn default_jitter_millis() -> u64 {
    1000
}

pub struct RateLimitBuilder {
    config: RateLimitConfig,
}

#[async_trait::async_trait]
impl Extension for RateLimitBuilder {
    type Config = RateLimitConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(Self::new(*config))
    }
}

impl RateLimitBuilder {
    pub fn new(config: RateLimitConfig) -> Self {
        assert!(config.burst > 0, "burst must be greater than 0");
        Self { config }
    }
    pub fn build(&self) -> RateLimit {
        let burst = NonZeroU32::new(self.config.burst).unwrap();
        let period = self.config.period;
        let jitter = Jitter::up_to(Duration::from_millis(self.config.jitter_millis));
        RateLimit::new(burst, period, jitter)
    }
}

#[derive(Clone)]
pub struct RateLimit {
    burst: NonZeroU32,
    period: Period,
    jitter: Jitter,
}

impl RateLimit {
    pub fn new(burst: NonZeroU32, period: Period, jitter: Jitter) -> Self {
        Self { burst, period, jitter }
    }

    pub fn make_copy(&self) -> Self {
        self.clone()
    }
}

impl<S> tower::Layer<S> for RateLimit {
    type Service = ConnectionRateLimit<S>;

    fn layer(&self, service: S) -> Self::Service {
        ConnectionRateLimit::new(service, self.burst, self.period, self.jitter)
    }
}

pub struct ConnectionRateLimit<S> {
    service: S,
    limiter: Arc<DefaultDirectRateLimiter>,
    jitter: Jitter,
}

impl<S> ConnectionRateLimit<S> {
    pub fn new(service: S, burst: NonZeroU32, period: Period, jitter: Jitter) -> Self {
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
