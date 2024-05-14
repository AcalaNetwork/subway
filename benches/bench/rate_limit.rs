use criterion::{criterion_group, Criterion};
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use governor::Jitter;
use governor::RateLimiter;
use jsonrpsee::server::middleware::rpc::RpcServiceT;
use jsonrpsee::types::Request;
use jsonrpsee::{MethodResponse, ResponsePayload};
use std::num::NonZeroU32;
use std::time::Duration;
use subway::extensions::rate_limit::{build_quota, ConnectionRateLimit, IpRateLimit};

#[derive(Clone)]
struct MockService;
impl RpcServiceT<'static> for MockService {
    type Future = BoxFuture<'static, MethodResponse>;

    fn call(&self, req: Request<'static>) -> Self::Future {
        async move { MethodResponse::response(req.id, ResponsePayload::success("ok"), 1024) }.boxed()
    }
}

pub fn connection_rate_limit(c: &mut Criterion) {
    let rate_limit = ConnectionRateLimit::new(
        MockService,
        NonZeroU32::new(1000).unwrap(),
        Duration::from_millis(1000),
        Jitter::up_to(Duration::from_millis(10)),
        Default::default(),
    );

    c.bench_function("rate_limit/connection_rate_limit", |b| {
        b.iter(|| rate_limit.call(Request::new("test".into(), None, jsonrpsee::types::Id::Number(1))))
    });
}

pub fn ip_rate_limit(c: &mut Criterion) {
    let burst = NonZeroU32::new(1000).unwrap();
    let quota = build_quota(burst, Duration::from_millis(1000));
    let limiter = RateLimiter::keyed(quota);
    let rate_limit = IpRateLimit::new(
        MockService,
        "::1".to_string(),
        std::sync::Arc::new(limiter),
        Jitter::up_to(Duration::from_millis(10)),
        Default::default(),
    );

    c.bench_function("rate_limit/ip_rate_limit", |b| {
        b.iter(|| rate_limit.call(Request::new("test".into(), None, jsonrpsee::types::Id::Number(1))))
    });
}

criterion_group!(rate_limit_benches, connection_rate_limit, ip_rate_limit);
