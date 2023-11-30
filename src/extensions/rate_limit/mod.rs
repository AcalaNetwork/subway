use futures::{future::BoxFuture, FutureExt};
use governor::{DefaultDirectRateLimiter, DefaultKeyedRateLimiter, Jitter, Quota, RateLimiter};
use jsonrpsee::{
    server::{middleware::rpc::RpcServiceT, types::Request},
    MethodResponse,
};
use serde::Deserialize;
use std::num::NonZeroU32;
use std::{sync::Arc, time::Duration};

use super::{Extension, ExtensionRegistry};

#[derive(Deserialize, Debug, Clone, Default, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitType {
    #[default]
    Ip,
    Connection,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct RateLimitConfig {
    pub rules: Vec<Rule>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct Rule {
    // burst is the maximum number of requests that can be made in a period
    pub burst: u32,
    // period is the period of time in which the burst is allowed
    #[serde(default = "default_period_secs")]
    pub period_secs: u64,
    // jitter_millis is the maximum amount of jitter to add to the rate limit
    // this is to prevent a thundering herd problem https://en.wikipedia.org/wiki/Thundering_herd_problem
    // e.g. if jitter_up_to_millis is 1000, then additional delay of random(0, 1000) milliseconds will be added
    #[serde(default = "default_jitter_up_to_millis")]
    pub jitter_up_to_millis: u64,
    #[serde(default)]
    pub apply_to: RateLimitType,
}

fn default_period_secs() -> u64 {
    1
}

fn default_jitter_up_to_millis() -> u64 {
    100
}

pub struct RateLimitBuilder {
    config: RateLimitConfig,
    ip_jitter: Option<Jitter>,
    ip_limiter: Option<Arc<DefaultKeyedRateLimiter<String>>>,
}

#[async_trait::async_trait]
impl Extension for RateLimitBuilder {
    type Config = RateLimitConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl RateLimitBuilder {
    pub fn new(config: RateLimitConfig) -> Self {
        // make sure there is at least one rule
        assert!(!config.rules.is_empty(), "must have at least one rule");
        // make sure there is at most one ip rule
        assert!(
            config.rules.iter().filter(|r| r.apply_to == RateLimitType::Ip).count() <= 1,
            "can only have one ip rule"
        );
        // make sure there is at most one connection rule
        assert!(
            config
                .rules
                .iter()
                .filter(|r| r.apply_to == RateLimitType::Connection)
                .count()
                <= 1,
            "can only have one connection rule"
        );
        // make sure all rules are valid
        for rule in config.rules.iter() {
            assert!(rule.burst > 0, "burst must be greater than 0");
            assert!(rule.period_secs > 0, "period_secs must be greater than 0");
        }

        if let Some(rule) = config.rules.iter().find(|r| r.apply_to == RateLimitType::Ip) {
            let burst = NonZeroU32::new(rule.burst).unwrap();
            let replenish_interval_ns = Duration::from_secs(rule.period_secs).as_nanos() / (burst.get() as u128);
            let quota = Quota::with_period(Duration::from_nanos(replenish_interval_ns as u64))
                .unwrap()
                .allow_burst(burst);
            Self {
                config: config.clone(),
                ip_jitter: Some(Jitter::up_to(Duration::from_millis(rule.jitter_up_to_millis))),
                ip_limiter: Some(Arc::new(RateLimiter::keyed(quota))),
            }
        } else {
            Self {
                config,
                ip_jitter: None,
                ip_limiter: None,
            }
        }
    }
    pub fn connection_limit(&self) -> Option<RateLimit> {
        if let Some(rule) = self
            .config
            .rules
            .iter()
            .find(|r| r.apply_to == RateLimitType::Connection)
        {
            let burst = NonZeroU32::new(rule.burst).unwrap();
            let period = Duration::from_secs(rule.period_secs);
            let jitter = Jitter::up_to(Duration::from_millis(rule.jitter_up_to_millis));
            Some(RateLimit::new(burst, period, jitter))
        } else {
            None
        }
    }
    pub fn ip_limit(&self, remote_ip: String) -> Option<IpRateLimitService> {
        self.ip_limiter.as_ref().map(|ip_limiter| {
            IpRateLimitService::new(remote_ip, ip_limiter.clone(), self.ip_jitter.unwrap_or_default())
        })
    }
}

#[derive(Clone)]
pub struct RateLimit {
    burst: NonZeroU32,
    period: Duration,
    jitter: Jitter,
}

impl RateLimit {
    pub fn new(burst: NonZeroU32, period: Duration, jitter: Jitter) -> Self {
        Self { burst, period, jitter }
    }
}

impl<S> tower::Layer<S> for RateLimit {
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

#[derive(Clone)]
pub struct IpRateLimitService {
    ip_addr: String,
    limiter: Arc<DefaultKeyedRateLimiter<String>>,
    jitter: Jitter,
}

impl IpRateLimitService {
    pub fn new(key: String, limiter: Arc<DefaultKeyedRateLimiter<String>>, jitter: Jitter) -> Self {
        Self {
            ip_addr: key,
            limiter,
            jitter,
        }
    }
}

impl<S> tower::Layer<S> for IpRateLimitService {
    type Service = IpRateLimitLayer<S>;

    fn layer(&self, service: S) -> Self::Service {
        IpRateLimitLayer::new(service, self.ip_addr.clone(), self.limiter.clone(), self.jitter)
    }
}

#[derive(Clone)]
pub struct IpRateLimitLayer<S> {
    service: S,
    ip_addr: String,
    limiter: Arc<DefaultKeyedRateLimiter<String>>,
    jitter: Jitter,
}

impl<S> IpRateLimitLayer<S> {
    pub fn new(service: S, ip_addr: String, limiter: Arc<DefaultKeyedRateLimiter<String>>, jitter: Jitter) -> Self {
        Self {
            service,
            ip_addr,
            limiter,
            jitter,
        }
    }
}

impl<'a, S> RpcServiceT<'a> for IpRateLimitLayer<S>
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
