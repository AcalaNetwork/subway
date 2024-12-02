use crate::{
    config::{Config, MergeStrategy, MiddlewaresConfig, RpcDefinitions, RpcSubscription},
    extensions::{
        client::{mock::TestServerBuilder, Client, ClientConfig},
        merge_subscription::MergeSubscriptionConfig,
        server::ServerConfig,
        ExtensionsConfig,
    },
    server,
};

#[tokio::test]
async fn upstream_error_propagate() {
    let subscribe_mock = "mock_sub";
    let unsubscribe_mock = "mock_unsub";
    let update_mock = "mock";

    let subscribe_merge_mock = "mock_merge_sub";
    let unsubscribe_merge_mock = "mock_merge_unsub";
    let update_merge_mock = "mock_merge";

    let mut builder = TestServerBuilder::new();

    builder.register_error_subscription(subscribe_mock, update_mock, unsubscribe_mock);
    builder.register_error_subscription(subscribe_merge_mock, unsubscribe_merge_mock, update_merge_mock);

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
                max_subscriptions_per_connection: 1024,
                max_batch_size: None,
                request_timeout_seconds: 120,
                http_methods: Vec::new(),
                cors: None,
            }),
            merge_subscription: Some(MergeSubscriptionConfig {
                keep_alive_seconds: Some(1),
            }),
            ..Default::default()
        },
        middlewares: MiddlewaresConfig {
            methods: vec![],
            subscriptions: vec!["merge_subscription".to_string(), "upstream".to_string()],
        },
        rpcs: RpcDefinitions {
            methods: vec![],
            subscriptions: vec![
                RpcSubscription {
                    subscribe: subscribe_mock.to_string(),
                    unsubscribe: unsubscribe_mock.to_string(),
                    name: update_mock.to_string(),
                    merge_strategy: None,
                },
                RpcSubscription {
                    subscribe: subscribe_merge_mock.to_string(),
                    unsubscribe: unsubscribe_merge_mock.to_string(),
                    name: update_merge_mock.to_string(),
                    merge_strategy: Some(MergeStrategy::Replace),
                },
            ],
            aliases: vec![],
        },
    };

    let subway_server = server::build(config).await.unwrap();
    let addr = subway_server.addr;

    let client = Client::with_endpoints([format!("ws://{addr}")]).unwrap();
    let result = client.subscribe(subscribe_mock, vec![], unsubscribe_mock).await;

    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("Inability to pay some fees (e.g. account balance too low)"));

    let result = client
        .subscribe(subscribe_merge_mock, vec![], unsubscribe_merge_mock)
        .await;

    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("Inability to pay some fees (e.g. account balance too low)"));

    // stop server
    subway_server.handle.stop().unwrap();
}
