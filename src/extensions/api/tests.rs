use jsonrpsee::server::ServerHandle;
use serde_json::json;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::mpsc;

use super::eth::EthApi;
use super::substrate::SubstrateApi;
use crate::extensions::client::{
    mock::{MockRequest, MockSubscription, TestServerBuilder},
    Client,
};

async fn create_server() -> (
    SocketAddr,
    ServerHandle,
    mpsc::Receiver<MockSubscription>,
    mpsc::Receiver<MockSubscription>,
    mpsc::Receiver<MockRequest>,
) {
    let mut builder = TestServerBuilder::new();

    let head_rx =
        builder.register_subscription("chain_subscribeNewHeads", "chain_newHead", "chain_unsubscribeNewHeads");

    let finalized_head_rx = builder.register_subscription(
        "chain_subscribeFinalizedHeads",
        "chain_finalizedHead",
        "chain_unsubscribeFinalizedHeads",
    );

    let block_hash_rx = builder.register_method("chain_getBlockHash");

    let (addr, server) = builder.build().await;

    (addr, server, head_rx, finalized_head_rx, block_hash_rx)
}

async fn create_eth_server() -> (
    SocketAddr,
    ServerHandle,
    mpsc::Receiver<MockSubscription>,
    mpsc::Receiver<MockRequest>,
) {
    let mut builder = TestServerBuilder::new();

    let subscription_rx = builder.register_subscription("eth_subscribe", "eth_subscription", "eth_unsubscribe");

    let block_rx = builder.register_method("eth_getBlockByNumber");

    let (addr, server) = builder.build().await;

    (addr, server, subscription_rx, block_rx)
}

async fn create_client() -> (
    Client,
    ServerHandle,
    mpsc::Receiver<MockSubscription>,
    mpsc::Receiver<MockSubscription>,
    mpsc::Receiver<MockRequest>,
) {
    let (addr, server, head_rx, finalized_head_rx, block_hash_rx) = create_server().await;

    let client = Client::new(
        [format!("ws://{addr}")],
        Duration::from_secs(1),
        Duration::from_secs(1),
        None,
        None,
    )
    .unwrap();

    (client, server, head_rx, finalized_head_rx, block_hash_rx)
}

async fn create_api() -> (
    SubstrateApi,
    ServerHandle,
    mpsc::Receiver<MockSubscription>,
    mpsc::Receiver<MockSubscription>,
    mpsc::Receiver<MockRequest>,
) {
    let (client, server, head_rx, finalized_head_rx, block_hash_rx) = create_client().await;
    let api = SubstrateApi::new(Arc::new(client), std::time::Duration::from_secs(100));

    (api, server, head_rx, finalized_head_rx, block_hash_rx)
}

#[tokio::test]
async fn get_head_finalized_head() {
    let (api, server, mut head_rx, mut finalized_head_rx, mut block_rx) = create_api().await;

    let head = api.get_head();
    let finalized_head = api.get_finalized_head();

    // access value before subscription is established

    let h1 = tokio::spawn(async move {
        assert_eq!(head.read().await, (json!("0xaa"), 0x01));
        // should be able to read it multiple times
        assert_eq!(head.read().await, (json!("0xaa"), 0x01));
    });

    let head_sub = head_rx.recv().await.unwrap();
    head_sub.send(json!({ "number": "0x01" })).await;

    {
        let req = block_rx.recv().await.unwrap();
        assert_eq!(req.params, json!([0x01]));
        req.respond(json!("0xaa"));
    }

    let finalized_head_sub = finalized_head_rx.recv().await.unwrap();
    finalized_head_sub.send(json!({ "number": "0x01" })).await;

    {
        let req = block_rx.recv().await.unwrap();
        assert_eq!(req.params, json!([0x01]));
        req.respond(json!("0xaa"));
    }

    // read after subscription is established

    let h2 = tokio::spawn(async move {
        let val = finalized_head.read().await;
        assert_eq!(val, (json!("0xaa"), 0x01));
    });

    // new head

    head_sub.send(json!({ "number": "0x02" })).await;

    {
        let req = block_rx.recv().await.unwrap();
        assert_eq!(req.params, json!([0x02]));
        req.respond(json!("0xbb"));
    }

    let finalized_head = api.get_finalized_head();
    // still old value
    assert_eq!(finalized_head.read().await, (json!("0xaa"), 0x01));

    // wait a bit for the value to be updated
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    let head = api.get_head();
    assert_eq!(head.read().await, (json!("0xbb"), 0x02));

    // new finalized head
    finalized_head_sub.send(json!({ "number": "0x03" })).await;
    finalized_head_sub.send(json!({ "number": "0x04" })).await;

    {
        let req = block_rx.recv().await.unwrap();
        assert_eq!(req.params, json!([0x03]));
        req.respond(json!("0xcc"));

        let req = block_rx.recv().await.unwrap();
        assert_eq!(req.params, json!([0x04]));
        req.respond(json!("0xdd"));
    }

    // wait a bit for the value to be updated
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    assert_eq!(finalized_head.read().await, (json!("0xdd"), 0x04));

    h1.await.unwrap();
    h2.await.unwrap();
    server.stop().unwrap();
}

#[tokio::test]
async fn rotate_endpoint_on_stale() {
    let (addr, server, mut head_rx, _, mut block_rx) = create_server().await;
    let (addr2, server2, mut head_rx2, _, mut block_rx2) = create_server().await;

    let client = Client::new(
        [format!("ws://{addr}"), format!("ws://{addr2}")],
        Duration::from_secs(1),
        Duration::from_secs(1),
        None,
        None,
    )
    .unwrap();
    let api = SubstrateApi::new(Arc::new(client), std::time::Duration::from_millis(100));

    let head = api.get_head();
    let h1 = tokio::spawn(async move {
        assert_eq!(head.read().await, (json!("0xabcd"), 0x1234));
    });

    // initial connection
    let head_sub = head_rx.recv().await.unwrap();
    head_sub.send(json!({ "number": "0x1234" })).await;
    {
        let req = block_rx.recv().await.unwrap();
        assert_eq!(req.params, json!([0x1234]));
        req.respond(json!("0xabcd"));
    }

    // wait a bit but before timeout
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // not stale
    head_sub.send(json!({ "number": "0x2345" })).await;
    {
        let req = block_rx.recv().await.unwrap();
        assert_eq!(req.params, json!([0x2345]));
        req.respond(json!("0xbcde"));
    }

    // wait a bit to process tasks
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    assert_eq!(api.get_head().read().await, (json!("0xbcde"), 0x2345));

    // wait for timeout
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // stale
    assert!(head_sub.sink.is_closed());

    // server 2
    let head_sub2 = head_rx2.recv().await.unwrap();
    head_sub2.send(json!({ "number": "0x4321" })).await;
    {
        let req = block_rx2.recv().await.unwrap();
        assert_eq!(req.params, json!([0x4321]));
        req.respond(json!("0xdcba"));
    }

    // wait a bit to process tasks
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    assert_eq!(api.get_head().read().await, (json!("0xdcba"), 0x4321));

    h1.await.unwrap();
    server.stop().unwrap();
    server2.stop().unwrap();
}

#[tokio::test]
async fn rotate_endpoint_on_head_mismatch() {
    let (addr1, server1, mut head_rx1, mut finalized_head_rx1, mut block_rx1) = create_server().await;
    let (addr2, server2, mut head_rx2, mut finalized_head_rx2, mut block_rx2) = create_server().await;

    let client = Client::new(
        [format!("ws://{addr1}"), format!("ws://{addr2}")],
        Duration::from_secs(1),
        Duration::from_secs(1),
        None,
        None,
    )
    .unwrap();

    let client = Arc::new(client);
    let api = SubstrateApi::new(client.clone(), std::time::Duration::from_millis(100));

    let head = api.get_head();
    let finalized_head = api.get_finalized_head();
    let h1 = tokio::spawn(async move {
        assert_eq!(head.read().await, (json!("0xaa"), 1));
        assert_eq!(finalized_head.read().await, (json!("0xaa"), 1));
    });

    // initial connection
    let head_sub = head_rx1.recv().await.unwrap();
    head_sub.send(json!({ "number": "0x01" })).await;
    {
        let req = block_rx1.recv().await.unwrap();
        assert_eq!(req.params, json!([0x01]));
        req.respond(json!("0xaa"));
    }

    let finalized_head_sub = finalized_head_rx1.recv().await.unwrap();
    finalized_head_sub.send(json!({ "number": "0x01" })).await;
    {
        let req = block_rx1.recv().await.unwrap();
        assert_eq!(req.params, json!([0x01]));
        req.respond(json!("0xaa"));
    }

    h1.await.unwrap();

    // not stale
    head_sub.send(json!({ "number": "0x02" })).await;
    {
        let req = block_rx1.recv().await.unwrap();
        assert_eq!(req.params, json!([0x02]));
        req.respond(json!("0xbb"));
    }
    finalized_head_sub.send(json!({ "number": "0x02" })).await;
    {
        let req = block_rx1.recv().await.unwrap();
        assert_eq!(req.params, json!([0x02]));
        req.respond(json!("0xbb"));
    }

    // wait a bit to process tasks
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    assert_eq!(api.get_finalized_head().read().await, (json!("0xbb"), 0x02));

    // stale server finalized head 1, trigger rotate endpoint
    finalized_head_sub.send(json!({ "number": "0x01" })).await;
    {
        let req = block_rx1.recv().await.unwrap();
        assert_eq!(req.params, json!([0x01]));
        req.respond(json!("0xaa"));
    }

    // wait a bit to process tasks
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    assert!(head_sub.sink.is_closed());
    assert!(finalized_head_sub.sink.is_closed());

    // current finalized head is still 2
    assert_eq!(api.get_finalized_head().read().await, (json!("0xbb"), 0x02));

    let head_sub2 = head_rx2.recv().await.unwrap();
    head_sub2.send(json!({ "number": "0x03" })).await;

    let req = block_rx2.recv().await.unwrap();
    assert_eq!(req.params, json!([0x03]));
    req.respond(json!("0xcc"));

    let finalized_head_sub2 = finalized_head_rx2.recv().await.unwrap();
    finalized_head_sub2.send(json!({ "number": "0x03" })).await;

    let req = block_rx2.recv().await.unwrap();
    assert_eq!(req.params, json!([0x03]));
    req.respond(json!("0xcc"));

    head_sub2.send(json!({ "number": "0x04" })).await;

    let req = block_rx2.recv().await.unwrap();
    assert_eq!(req.params, json!([0x04]));
    req.respond(json!("0xdd"));

    // wait a bit to process tasks
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    // current head=4 and finalized_head=3
    assert_eq!(api.get_head().read().await, (json!("0xdd"), 0x04));
    assert_eq!(api.get_finalized_head().read().await, (json!("0xcc"), 0x03));

    server1.stop().unwrap();
    server2.stop().unwrap();
}

#[tokio::test]
async fn substrate_background_tasks_abort_on_drop() {
    let (addr, _server, mut head_rx, mut finalized_head_rx, _) = create_server().await;
    let client = Arc::new(
        Client::new(
            [format!("ws://{addr}")],
            Duration::from_secs(1),
            Duration::from_secs(1),
            None,
            None,
        )
        .unwrap(),
    );
    let api = SubstrateApi::new(client, std::time::Duration::from_millis(100));

    // background tasks started
    let head_sub = head_rx.recv().await.unwrap();
    let finalized_head_sub = finalized_head_rx.recv().await.unwrap();

    drop(api);

    // wait a bit to abort tasks
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // background tasks aborted, subscription closed
    assert!(head_sub.sink.is_closed());
    assert!(finalized_head_sub.sink.is_closed());
}

#[tokio::test]
async fn eth_background_tasks_abort_on_drop() {
    let (addr, _server, mut subscription_rx, mut block_rx) = create_eth_server().await;
    let client = Arc::new(
        Client::new(
            [format!("ws://{addr}")],
            Duration::from_secs(1),
            Duration::from_secs(1),
            None,
            None,
        )
        .unwrap(),
    );

    let api = EthApi::new(client, std::time::Duration::from_millis(100));

    // background tasks started
    let block_req = block_rx.recv().await.unwrap();
    block_req.respond(json!({ "number": "0x01", "hash": "0xaa"}));

    let head_sub = subscription_rx.recv().await.unwrap();
    let finalized_sub = subscription_rx.recv().await.unwrap();

    drop(api);

    // wait a bit to abort tasks
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // background tasks aborted, subscription closed
    assert!(head_sub.sink.is_closed());
    assert!(finalized_sub.sink.is_closed());
}
