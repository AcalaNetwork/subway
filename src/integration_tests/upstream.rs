use crate::{
    config::{Config, MiddlewaresConfig, RpcDefinitions, RpcSubscription},
    extensions::{
        client::{mock::TestServerBuilder, Client, ClientConfig},
        server::ServerConfig,
        ExtensionsConfig,
    },
    server::start_server,
};

#[tokio::test]
async fn upstream_error_propagate() {
    let subscribe_mock = "mock_sub";
    let unsubscribe_mock = "mock_unsub";
    let update_mock = "mock";

    let mut builder = TestServerBuilder::new();

    builder.register_error_subscription(subscribe_mock, update_mock, unsubscribe_mock);

    let (addr, _upstream_handle) = builder.build().await;

    let config = Config {
        extensions: ExtensionsConfig {
            client: Some(ClientConfig {
                endpoints: vec![format!("ws://{addr}")],
                shuffle_endpoints: false,
            }),
            server: Some(ServerConfig {
                listen_address: "0.0.0.0".to_string(),
                port: 0,
                max_connections: 10,
                request_timeout_seconds: 120,
                http_methods: Vec::new(),
            }),
            ..Default::default()
        },
        middlewares: MiddlewaresConfig {
            methods: vec![],
            subscriptions: vec!["upstream".to_string()],
        },
        rpcs: RpcDefinitions {
            methods: vec![],
            subscriptions: vec![RpcSubscription {
                subscribe: subscribe_mock.to_string(),
                unsubscribe: unsubscribe_mock.to_string(),
                name: update_mock.to_string(),
                merge_strategy: None,
            }],
            aliases: vec![],
        },
    };

    let subway_server = start_server(config).await.unwrap();
    let addr = subway_server.addr;

    let client = Client::with_endpoints([format!("ws://{addr}")]).unwrap();
    let result = client.subscribe(subscribe_mock, vec![], unsubscribe_mock).await;

    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("Inability to pay some fees (e.g. account balance too low)"));

    // stop server
    subway_server.handle.stop().unwrap();
}
