use criterion::*;
use futures::{future::join_all, stream::FuturesUnordered};
use futures_util::FutureExt;
use jsonrpsee::core::params::BatchRequestBuilder;
use pprof::criterion::{Output, PProfProfiler};
use std::{sync::Arc, time::Duration};
use tokio::runtime::Runtime as TokioRuntime;

mod rate_limit;

use helpers::{
    client::{rpc_params, ws_client, ws_handshake, ClientT, HeaderMap, SubscriptionClientT},
    ASYNC_INJECT_CALL, KIB, SUB_METHOD_NAME, UNSUB_METHOD_NAME,
};

use subway::{
    config::{Config, MergeStrategy, MethodParam, MiddlewaresConfig, RpcDefinitions, RpcMethod, RpcSubscription},
    extensions::{api::SubstrateApiConfig, client::ClientConfig, server::ServerConfig, ExtensionsConfig},
};

mod helpers;

fn measurement_slow() -> Duration {
    std::env::var("SLOW_MEASUREMENT_TIME").map_or(Duration::from_secs(60), |val| {
        Duration::from_secs(val.parse().expect("SLOW_SAMPLE_TIME must be an integer"))
    })
}

fn measurement_mid() -> Duration {
    std::env::var("MEASUREMENT_TIME").map_or(Duration::from_secs(10), |val| {
        Duration::from_secs(val.parse().expect("SAMPLE_TIME must be an integer"))
    })
}

criterion_group!(
    name = sync_benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = SyncBencher::websocket_benches
);
criterion_group!(
    name = sync_benches_mid;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None))).measurement_time(measurement_mid());
    targets = SyncBencher::websocket_benches_mid
);
criterion_group!(
    name = sync_benches_slow;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None))).measurement_time(measurement_slow());
    targets = SyncBencher::websocket_benches_slow
);
criterion_group!(
    name = async_benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = AsyncBencher::websocket_benches
);
criterion_group!(
    name = async_benches_mid;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None))).measurement_time(measurement_mid());
    targets = AsyncBencher::websocket_benches_mid
);
criterion_group!(
    name = async_benches_slow;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None))).measurement_time(measurement_slow());
    targets = AsyncBencher::websocket_benches_slow
);
criterion_group!(
    name = subscriptions;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = AsyncBencher::subscriptions
);
criterion_group!(
    name = async_benches_inject;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = AsyncBencher::websocket_benches_inject
);

criterion_main!(
    sync_benches,
    sync_benches_mid,
    sync_benches_slow,
    async_benches,
    async_benches_mid,
    async_benches_slow,
    subscriptions,
    async_benches_inject,
    rate_limit::rate_limit_benches,
);

const SERVER_ONE_ENDPOINT: &str = "127.0.0.1:9955";
const SERVER_TWO_ENDPOINT: &str = "127.0.0.1:9966";

const SUBWAY_SERVER_ADDR: &str = "127.0.0.1";
const SUBWAY_SERVER_PORT: u16 = 9944;

#[derive(Debug, Clone, Copy)]
enum RequestType {
    Sync,
    Async,
}

impl RequestType {
    fn methods(self) -> [&'static str; 3] {
        match self {
            RequestType::Sync => crate::helpers::SYNC_METHODS,
            RequestType::Async => crate::helpers::ASYNC_METHODS,
        }
    }

    fn group_name(self, name: &str) -> String {
        let request_type_name = match self {
            RequestType::Sync => "sync",
            RequestType::Async => "async",
        };
        format!("{}/{}", request_type_name, name)
    }
}

trait RequestBencher {
    const REQUEST_TYPE: RequestType;

    fn websocket_benches(crit: &mut Criterion) {
        let rt = TokioRuntime::new().unwrap();
        let (_url1, _server1) = rt.block_on(helpers::ws_server(rt.handle().clone(), SERVER_ONE_ENDPOINT));
        let (_url2, _server2) = rt.block_on(helpers::ws_server(rt.handle().clone(), SERVER_TWO_ENDPOINT));
        let subway_server = rt.block_on(server());
        let url = format!("ws://{}", subway_server.addr);
        ws_custom_headers_handshake(&rt, crit, &url, "ws_custom_headers_handshake", Self::REQUEST_TYPE);
        ws_concurrent_conn_calls(
            &rt,
            crit,
            &url,
            "ws_concurrent_conn_calls",
            Self::REQUEST_TYPE,
            &[2, 4, 8],
        );
        let client = Arc::new(rt.block_on(ws_client(&url)));
        round_trip(&rt, crit, client.clone(), "ws_round_trip", Self::REQUEST_TYPE);
        batch_round_trip(&rt, crit, client, "ws_batch_requests", Self::REQUEST_TYPE);
    }

    fn websocket_benches_mid(crit: &mut Criterion) {
        let rt = TokioRuntime::new().unwrap();
        let (_url1, _server1) = rt.block_on(helpers::ws_server(rt.handle().clone(), SERVER_ONE_ENDPOINT));
        let (_url2, _server2) = rt.block_on(helpers::ws_server(rt.handle().clone(), SERVER_TWO_ENDPOINT));
        let subway_server = rt.block_on(server());
        let url = format!("ws://{}", subway_server.addr);
        ws_concurrent_conn_calls(
            &rt,
            crit,
            &url,
            "ws_concurrent_conn_calls",
            Self::REQUEST_TYPE,
            &[16, 32, 64],
        );
        ws_concurrent_conn_subs(
            &rt,
            crit,
            &url,
            "ws_concurrent_conn_subs",
            Self::REQUEST_TYPE,
            &[16, 32, 64],
        );
    }

    fn websocket_benches_slow(crit: &mut Criterion) {
        let rt = TokioRuntime::new().unwrap();
        let (_url1, _server1) = rt.block_on(helpers::ws_server(rt.handle().clone(), SERVER_ONE_ENDPOINT));
        let (_url2, _server2) = rt.block_on(helpers::ws_server(rt.handle().clone(), SERVER_TWO_ENDPOINT));
        let subway_server = rt.block_on(server());
        let url = format!("ws://{}", subway_server.addr);
        ws_concurrent_conn_calls(
            &rt,
            crit,
            &url,
            "ws_concurrent_conn_calls",
            Self::REQUEST_TYPE,
            &[128, 256, 512],
        );
    }

    fn subscriptions(crit: &mut Criterion) {
        let rt = TokioRuntime::new().unwrap();
        let (_url1, _server1) = rt.block_on(helpers::ws_server(rt.handle().clone(), SERVER_ONE_ENDPOINT));
        let (_url2, _server2) = rt.block_on(helpers::ws_server(rt.handle().clone(), SERVER_TWO_ENDPOINT));
        let subway_server = rt.block_on(server());
        let url = format!("ws://{}", subway_server.addr);
        let client = Arc::new(rt.block_on(ws_client(&url)));
        sub_round_trip(&rt, crit, client, "subscriptions");
    }

    fn websocket_benches_inject(crit: &mut Criterion) {
        let rt = TokioRuntime::new().unwrap();
        let (_url1, _server1) = rt.block_on(helpers::ws_server(rt.handle().clone(), SERVER_ONE_ENDPOINT));
        let (_url2, _server2) = rt.block_on(helpers::ws_server(rt.handle().clone(), SERVER_TWO_ENDPOINT));
        let subway_server = rt.block_on(server());
        let url = format!("ws://{}", subway_server.addr);
        let client = Arc::new(rt.block_on(ws_client(&url)));
        ws_inject_calls(&rt, crit, client, "ws_inject_calls", Self::REQUEST_TYPE);
    }
}

pub struct AsyncBencher;
impl RequestBencher for AsyncBencher {
    const REQUEST_TYPE: RequestType = RequestType::Async;
}

pub struct SyncBencher;
impl RequestBencher for SyncBencher {
    const REQUEST_TYPE: RequestType = RequestType::Sync;
}

fn config() -> Config {
    Config {
        extensions: ExtensionsConfig {
            client: Some(ClientConfig {
                endpoints: vec![
                    format!("ws://{}", SERVER_ONE_ENDPOINT),
                    format!("ws://{}", SERVER_TWO_ENDPOINT),
                ],
                shuffle_endpoints: false,
            }),
            server: Some(ServerConfig {
                listen_address: SUBWAY_SERVER_ADDR.to_string(),
                port: SUBWAY_SERVER_PORT,
                max_connections: 1024 * 1024,
                max_subscriptions_per_connection: 1024,
                max_batch_size: None,
                request_timeout_seconds: 120,
                http_methods: Vec::new(),
                cors: None,
            }),
            substrate_api: Some(SubstrateApiConfig {
                stale_timeout_seconds: 5_000,
            }),
            ..Default::default()
        },
        middlewares: MiddlewaresConfig {
            methods: vec!["inject_params".to_string(), "upstream".to_string()],
            subscriptions: vec!["upstream".to_string()],
        },
        rpcs: RpcDefinitions {
            methods: vec![
                RpcMethod {
                    method: helpers::SYNC_FAST_CALL.to_string(),
                    params: vec![],
                    response: None,
                    cache: None,
                    delay_ms: None,
                    rate_limit_weight: 1,
                },
                RpcMethod {
                    method: helpers::ASYNC_FAST_CALL.to_string(),
                    params: vec![],
                    response: None,
                    cache: None,
                    delay_ms: None,
                    rate_limit_weight: 1,
                },
                RpcMethod {
                    method: helpers::SYNC_MEM_CALL.to_string(),
                    params: vec![],
                    response: None,
                    cache: None,
                    delay_ms: None,
                    rate_limit_weight: 1,
                },
                RpcMethod {
                    method: helpers::ASYNC_MEM_CALL.to_string(),
                    params: vec![],
                    response: None,
                    cache: None,
                    delay_ms: None,
                    rate_limit_weight: 1,
                },
                RpcMethod {
                    method: helpers::SYNC_SLOW_CALL.to_string(),
                    params: vec![],
                    response: None,
                    cache: None,
                    delay_ms: None,
                    rate_limit_weight: 1,
                },
                RpcMethod {
                    method: helpers::ASYNC_SLOW_CALL.to_string(),
                    params: vec![],
                    response: None,
                    cache: None,
                    delay_ms: None,
                    rate_limit_weight: 1,
                },
                RpcMethod {
                    method: helpers::ASYNC_INJECT_CALL.to_string(),
                    params: vec![
                        MethodParam {
                            name: "pho".to_string(),
                            ty: "u64".to_string(),
                            optional: false,
                            inject: false,
                        },
                        MethodParam {
                            name: "bar".to_string(),
                            ty: "BlockNumber".to_string(),
                            optional: true,
                            inject: true,
                        },
                    ],
                    response: None,
                    cache: None,
                    delay_ms: None,
                    rate_limit_weight: 1,
                },
            ],
            subscriptions: vec![RpcSubscription {
                subscribe: helpers::SUB_METHOD_NAME.to_string(),
                unsubscribe: helpers::UNSUB_METHOD_NAME.to_string(),
                name: helpers::SUB_METHOD_NAME.to_string(),
                merge_strategy: Some(MergeStrategy::Replace),
            }],
            aliases: vec![],
        },
    }
}

async fn server() -> subway::server::SubwayServerHandle {
    let config = config();
    subway::server::build(config).await.unwrap()
}

fn ws_concurrent_conn_calls(
    rt: &TokioRuntime,
    crit: &mut Criterion,
    url: &str,
    name: &str,
    request: RequestType,
    concurrent_conns: &[usize],
) {
    let fast_call = request.methods()[0];
    assert!(fast_call.starts_with("fast_call"));

    let bench_name = format!("{}/{}", name, fast_call);
    let mut group = crit.benchmark_group(request.group_name(&bench_name));
    for conns in concurrent_conns.iter() {
        group.bench_function(format!("{}", conns), |b| {
            b.to_async(rt).iter_with_setup(
                || {
                    let clients = (0..*conns).map(|_| ws_client(url)).collect::<Vec<_>>();
                    // We have to use `block_in_place` here since `b.to_async(rt)` automatically enters the
                    // runtime context and simply calling `block_on` here will cause the code to panic.
                    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(join_all(clients)))
                },
                |clients| async {
                    let tasks = clients.into_iter().map(|client| {
                        rt.spawn(async move {
                            let futs = FuturesUnordered::new();

                            for _ in 0..10 {
                                futs.push(client.request::<String, _>(fast_call, rpc_params![]));
                            }

                            join_all(futs).await;
                        })
                    });
                    join_all(tasks).await;
                },
            )
        });
    }
    group.finish();
}

// As this is so slow only fast calls are executed in this benchmark.
fn ws_concurrent_conn_subs(
    rt: &TokioRuntime,
    crit: &mut Criterion,
    url: &str,
    name: &str,
    request: RequestType,
    concurrent_conns: &[usize],
) {
    let mut group = crit.benchmark_group(request.group_name(name));
    for conns in concurrent_conns.iter() {
        group.bench_function(format!("{}", conns), |b| {
            b.to_async(rt).iter_with_setup(
                || {
                    let clients = (0..*conns).map(|_| ws_client(url)).collect::<Vec<_>>();
                    // We have to use `block_in_place` here since `b.to_async(rt)` automatically enters the
                    // runtime context and simply calling `block_on` here will cause the code to panic.
                    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(join_all(clients)))
                },
                |clients| async {
                    let tasks = clients.into_iter().map(|client| {
                        rt.spawn(async move {
                            let futs = FuturesUnordered::new();

                            for _ in 0..10 {
                                let fut = client
                                    .subscribe::<String, _>(SUB_METHOD_NAME, rpc_params![], UNSUB_METHOD_NAME)
                                    .then(|sub| async move {
                                        let mut s = sub.unwrap();

                                        s.next().await.unwrap().unwrap()
                                    });

                                futs.push(Box::pin(fut));
                            }

                            join_all(futs).await;
                        })
                    });
                    join_all(tasks).await;
                },
            )
        });
    }
    group.finish();
}

/// Bench WS handshake with different header sizes.
fn ws_custom_headers_handshake(rt: &TokioRuntime, crit: &mut Criterion, url: &str, name: &str, request: RequestType) {
    let mut group = crit.benchmark_group(request.group_name(name));
    for header_size in [0, KIB, 2 * KIB, 4 * KIB] {
        group.bench_function(format!("{}kb", header_size / KIB), |b| {
            b.to_async(rt).iter(|| async move {
                let mut headers = HeaderMap::new();
                if header_size != 0 {
                    headers.insert("key", "A".repeat(header_size).parse().unwrap());
                }

                ws_handshake(url, headers).await;
            })
        });
    }
    group.finish();
}

fn round_trip(rt: &TokioRuntime, crit: &mut Criterion, client: Arc<impl ClientT>, name: &str, request: RequestType) {
    for method in request.methods() {
        let bench_name = format!("{}/{}", name, method);
        crit.bench_function(&request.group_name(&bench_name), |b| {
            b.to_async(rt).iter(|| async {
                black_box(client.request::<String, _>(method, rpc_params![]).await.unwrap());
            })
        });
    }
}

/// Benchmark batch_requests over batch sizes of 2, 5, 10, 50 and 100 RPCs in each batch.
fn batch_round_trip(
    rt: &TokioRuntime,
    crit: &mut Criterion,
    client: Arc<impl ClientT>,
    name: &str,
    request: RequestType,
) {
    let fast_call = request.methods()[0];
    assert!(fast_call.starts_with("fast_call"));

    let bench_name = format!("{}/{}", name, fast_call);
    let mut group = crit.benchmark_group(request.group_name(&bench_name));
    for batch_size in [2, 5, 10, 50, 100usize].iter() {
        let mut batch = BatchRequestBuilder::new();
        for _ in 0..*batch_size {
            batch.insert(fast_call, rpc_params![]).unwrap();
        }

        group.throughput(Throughput::Elements(*batch_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(batch_size), batch_size, |b, _| {
            b.to_async(rt)
                .iter(|| async { client.batch_request::<String>(batch.clone()).await.unwrap() })
        });
    }
    group.finish();
}

fn sub_round_trip(rt: &TokioRuntime, crit: &mut Criterion, client: Arc<impl SubscriptionClientT>, name: &str) {
    let mut group = crit.benchmark_group(name);
    group.bench_function("subscribe", |b| {
        b.to_async(rt).iter_with_large_drop(|| async {
            black_box(
                client
                    .subscribe::<String, _>(SUB_METHOD_NAME, rpc_params![], UNSUB_METHOD_NAME)
                    .await
                    .unwrap(),
            );
        })
    });
    group.bench_function("subscribe_response", |b| {
        b.to_async(rt).iter_with_setup(
            || {
                // We have to use `block_in_place` here since `b.to_async(rt)` automatically enters the
                // runtime context and simply calling `block_on` here will cause the code to panic.
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        client
                            .subscribe::<String, _>(SUB_METHOD_NAME, rpc_params![], UNSUB_METHOD_NAME)
                            .await
                            .unwrap()
                    })
                })
            },
            |mut sub| async move {
                black_box(sub.next().await.transpose().unwrap());
                // Note that this benchmark will include costs for measuring `drop` for subscription,
                // since it's not possible to combine both `iter_with_setup` and `iter_with_large_drop`.
                // To estimate pure cost of method, one should subtract the result of `unsub` bench
                // from this one.
            },
        )
    });
    group.bench_function("unsub", |b| {
        b.iter_with_setup(
            || {
                rt.block_on(async {
                    client
                        .subscribe::<String, _>(SUB_METHOD_NAME, rpc_params![], UNSUB_METHOD_NAME)
                        .await
                        .unwrap()
                })
            },
            |sub| {
                // Subscription will be closed inside of the drop impl.
                // Actually, it just sends a notification about object being closed,
                // but it's still important to know that drop impl is not too expensive.
                drop(black_box(sub));
            },
        )
    });
}

fn ws_inject_calls(
    rt: &TokioRuntime,
    crit: &mut Criterion,
    client: Arc<impl ClientT>,
    name: &str,
    request: RequestType,
) {
    let method = ASYNC_INJECT_CALL;
    let bench_name = format!("{}/{}", name, method);
    crit.bench_function(&request.group_name(&bench_name), |b| {
        b.to_async(rt).iter(|| async {
            black_box(client.request::<String, _>(method, rpc_params![0_u64]).await.unwrap());
        })
    });
}
