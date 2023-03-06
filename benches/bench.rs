use criterion::*;
use futures::{future::join_all, stream::FuturesUnordered};
use jsonrpsee::{core::Error as RpcError, server::ServerHandle, RpcModule};
use tokio::runtime::Runtime as TokioRuntime;
use pprof::criterion::{Output, PProfProfiler};
use std::time::Duration;

use subway::client::create_client;
use subway::config::{Config, RpcDefinitions, RpcMethod, ServerConfig};
use subway::server::start_server;

use fixed_client::{ws_client, ClientT};

fn measurement_mid() -> Duration {
	std::env::var("MEASUREMENT_TIME").map_or(Duration::from_secs(10), |val| {
		Duration::from_secs(val.parse().expect("SAMPLE_TIME must be an integer"))
	})
}

criterion_group!(
    name = async_benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None))).measurement_time(measurement_mid());
    targets = Bencher::benches
);
criterion_main!(
    async_benches,
);

const FAST_CAll: &str = "fast_call";

const SERVER_ONE_ENDPOINT: &str = "0.0.0.0:442";
const SERVER_TWO_ENDPOINT: &str = "0.0.0.0:443";

const SUBWAY_SERVER_ADDR: &str = "0.0.0.0";
const SUBWAY_SERVER_PORT: u16 = 9944;

pub struct Bencher;

impl Bencher {
    fn benches(crit: &mut Criterion) {
        let rt = TokioRuntime::new().unwrap();
        rt.block_on(ws_server(SERVER_ONE_ENDPOINT));
        rt.block_on(ws_server(SERVER_TWO_ENDPOINT));
        let (_url, _server) = rt.block_on(server());
        ws_fast_call(&rt, crit, &[2, 4, 8]);
    }
}

fn ws_fast_call(rt: &TokioRuntime, crit: &mut Criterion, concurrent_conns: &[usize]) {
    let mut group = crit.benchmark_group("ws_fast_call");
    for conns in concurrent_conns.iter() {
        group.bench_function(format!("{}", conns), |b| {
            b.to_async(rt).iter_with_setup(
                || {
                    let mut clients = Vec::new();
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            for _ in 0..*conns {
                                clients.push(
                                    ws_client(
                                        format!("ws://{}:{}", SUBWAY_SERVER_ADDR, SUBWAY_SERVER_PORT)
                                            .as_str(),
                                    )
                                    .await,
                                );
                            }
                        })
                    });

                    clients
                },
                |clients| async {
                    let tasks = clients.into_iter().map(|client| {
                        rt.spawn(async move {
                            let futs = FuturesUnordered::new();
                            for _ in 0..100 {
                                futs.push(client.request::<String>(FAST_CAll, None));
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

fn config() -> Config {
    Config {
        endpoints: vec![
            SERVER_ONE_ENDPOINT.to_string(),
            SERVER_TWO_ENDPOINT.to_string(),
        ],
        stale_timeout_seconds: 60,
        server: ServerConfig {
            listen_address: SUBWAY_SERVER_ADDR.to_string(),
            port: SUBWAY_SERVER_PORT,
            max_connections: 2000,
        },
        rpcs: RpcDefinitions {
            methods: vec![RpcMethod {
                method: FAST_CAll.to_string(),
                params: vec![],
                cache: 0,
            }],
            subscriptions: vec![],
            aliases: vec![],
        },
    }
}

async fn server() -> (String, tokio::task::JoinHandle<()>) {
    let config = config();
    let client = create_client(&config).await.unwrap();
    let (addr, handle) = start_server(&config, client).await.unwrap();
    (format!("ws://{}", addr), handle)
}

async fn ws_server(url: &str) -> (String, ServerHandle) {
    use jsonrpsee::server::ServerBuilder;
    
    println!("Starting server at {}", url);

    let server = ServerBuilder::default()
        .max_request_body_size(u32::MAX)
        .max_response_body_size(u32::MAX)
        .max_connections(10 * 1024)
        .build(url)
        .await
        .unwrap();
    let module = gen_rpc_module();
    let addr = format!("ws://{}", server.local_addr().unwrap());
    let handle = server.start(module).unwrap();
    (addr, handle)
}

fn gen_rpc_module() -> RpcModule<()> {
    let mut module = jsonrpsee::RpcModule::new(());
    module
        .register_async_method(FAST_CAll, |_, _| async { Result::<_, RpcError>::Ok("fc") })
        .unwrap();
    module
}

mod fixed_client {
    pub use jsonrpsee_v0_15::core::client::ClientT;
    use jsonrpsee_v0_15::ws_client::{WsClient, WsClientBuilder};

    pub(crate) async fn ws_client(url: &str) -> WsClient {
        WsClientBuilder::default()
            .max_request_body_size(u32::MAX)
            .max_concurrent_requests(1024 * 1024)
            .build(url)
            .await
            .unwrap()
    }
}
