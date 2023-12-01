use governor::{DefaultKeyedRateLimiter, Jitter, Quota, RateLimiter};
use serde::Deserialize;
use std::num::NonZeroU32;
use std::{sync::Arc, time::Duration};

use super::{Extension, ExtensionRegistry};

mod connection;
mod ip;

pub use connection::{ConnectionRateLimit, ConnectionRateLimitLayer};
pub use ip::{IpRateLimit, IpRateLimitLayer};

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
            let quota = build_quota(burst, Duration::from_secs(rule.period_secs));
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
    pub fn connection_limit(&self) -> Option<ConnectionRateLimitLayer> {
        if let Some(rule) = self
            .config
            .rules
            .iter()
            .find(|r| r.apply_to == RateLimitType::Connection)
        {
            let burst = NonZeroU32::new(rule.burst).unwrap();
            let period = Duration::from_secs(rule.period_secs);
            let jitter = Jitter::up_to(Duration::from_millis(rule.jitter_up_to_millis));
            Some(ConnectionRateLimitLayer::new(burst, period, jitter))
        } else {
            None
        }
    }
    pub fn ip_limit(&self, remote_ip: String) -> Option<IpRateLimitLayer> {
        self.ip_limiter
            .as_ref()
            .map(|ip_limiter| IpRateLimitLayer::new(remote_ip, ip_limiter.clone(), self.ip_jitter.unwrap_or_default()))
    }
}

pub fn build_quota(burst: NonZeroU32, period: Duration) -> Quota {
    let replenish_interval_ns = period.as_nanos() / (burst.get() as u128);
    Quota::with_period(Duration::from_nanos(replenish_interval_ns as u64))
        .unwrap()
        .allow_burst(burst)
}
