use std::net::SocketAddr;
use std::sync::Arc;

use jsonrpsee::{server::ServerHandle, SubscriptionMessage, SubscriptionSink};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use super::substrate::SubstrateApi;
use super::*;

use crate::extensions::client::{mock::TestServerBuilder, Client};

async fn create_server() -> (
    SocketAddr,
    ServerHandle,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
    mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
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

async fn create_client() -> (
    Client,
    ServerHandle,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
    mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
) {
    let (addr, server, head_rx, finalized_head_rx, block_hash_rx) = create_server().await;

    let client = Client::new([format!("ws://{addr}")]).unwrap();

    (client, server, head_rx, finalized_head_rx, block_hash_rx)
}

async fn create_api() -> (
    SubstrateApi,
    ServerHandle,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
    mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
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

    let (_, head_sink) = head_rx.recv().await.unwrap();
    head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x01" })).unwrap())
        .await
        .unwrap();

    {
        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x01]));
        tx.send(json!("0xaa")).unwrap();
    }

    let (_, finalized_head_sink) = finalized_head_rx.recv().await.unwrap();
    finalized_head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x01" })).unwrap())
        .await
        .unwrap();

    {
        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x01]));
        tx.send(json!("0xaa")).unwrap();
    }

    // read after subscription is established

    let h2 = tokio::spawn(async move {
        let val = finalized_head.read().await;
        assert_eq!(val, (json!("0xaa"), 0x01));
    });

    // new head

    head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x02" })).unwrap())
        .await
        .unwrap();

    {
        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x02]));
        tx.send(json!("0xbb")).unwrap();
    }

    let finalized_head = api.get_finalized_head();
    // still old value
    assert_eq!(finalized_head.read().await, (json!("0xaa"), 0x01));

    // wait a bit for the value to be updated
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    let head = api.get_head();
    assert_eq!(head.read().await, (json!("0xbb"), 0x02));

    // new finalized head
    finalized_head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x03" })).unwrap())
        .await
        .unwrap();
    finalized_head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x04" })).unwrap())
        .await
        .unwrap();

    {
        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x03]));
        tx.send(json!("0xcc")).unwrap();

        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x04]));
        tx.send(json!("0xdd")).unwrap();
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

    let client = Client::new([format!("ws://{addr}"), format!("ws://{addr2}")]).unwrap();
    let api = SubstrateApi::new(Arc::new(client), std::time::Duration::from_millis(100));

    let head = api.get_head();
    let h1 = tokio::spawn(async move {
        assert_eq!(head.read().await, (json!("0xabcd"), 0x1234));
    });

    // initial connection
    let (_, head_sink) = head_rx.recv().await.unwrap();
    head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x1234" })).unwrap())
        .await
        .unwrap();
    {
        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x1234]));
        tx.send(json!("0xabcd")).unwrap();
    }

    // wait a bit but before timeout
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // not stale
    head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x2345" })).unwrap())
        .await
        .unwrap();
    {
        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x2345]));
        tx.send(json!("0xbcde")).unwrap();
    }

    // wait a bit to process tasks
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    assert_eq!(api.get_head().read().await, (json!("0xbcde"), 0x2345));

    // wait for timeout
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // stale
    assert!(head_sink.is_closed());

    // server 2
    let (_, head_sink2) = head_rx2.recv().await.unwrap();
    head_sink2
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x4321" })).unwrap())
        .await
        .unwrap();
    {
        let (params, tx) = block_rx2.recv().await.unwrap();
        assert_eq!(params, json!([0x4321]));
        tx.send(json!("0xdcba")).unwrap();
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

    let client = Client::new([format!("ws://{addr1}"), format!("ws://{addr2}")]).unwrap();

    let client = Arc::new(client);
    // TODO: investigate why it takes a while to connect to another endpoint
    let api = SubstrateApi::new(client.clone(), std::time::Duration::from_millis(5_000));

    let head = api.get_head();
    let finalized_head = api.get_finalized_head();
    let h1 = tokio::spawn(async move {
        assert_eq!(head.read().await, (json!("0xaa"), 1));
        assert_eq!(finalized_head.read().await, (json!("0xaa"), 1));
    });

    // initial connection
    let (_, head_sink) = head_rx1.recv().await.unwrap();
    head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x01" })).unwrap())
        .await
        .unwrap();
    {
        let (params, tx) = block_rx1.recv().await.unwrap();
        assert_eq!(params, json!([0x01]));
        tx.send(json!("0xaa")).unwrap();
    }

    let (_, finalized_head_sink) = finalized_head_rx1.recv().await.unwrap();
    finalized_head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x01" })).unwrap())
        .await
        .unwrap();
    {
        let (params, tx) = block_rx1.recv().await.unwrap();
        assert_eq!(params, json!([0x01]));
        tx.send(json!("0xaa")).unwrap();
    }

    h1.await.unwrap();

    // not stale
    head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x02" })).unwrap())
        .await
        .unwrap();
    {
        let (params, tx) = block_rx1.recv().await.unwrap();
        assert_eq!(params, json!([0x02]));
        tx.send(json!("0xbb")).unwrap();
    }
    finalized_head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x02" })).unwrap())
        .await
        .unwrap();
    {
        let (params, tx) = block_rx1.recv().await.unwrap();
        assert_eq!(params, json!([0x02]));
        tx.send(json!("0xbb")).unwrap();
    }

    // wait a bit to process tasks
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    assert_eq!(api.get_finalized_head().read().await, (json!("0xbb"), 0x02));

    // stale server finalized head 1, trigger rotate endpoint
    finalized_head_sink
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x01" })).unwrap())
        .await
        .unwrap();
    {
        let (params, tx) = block_rx1.recv().await.unwrap();
        assert_eq!(params, json!([0x01]));
        tx.send(json!("0xaa")).unwrap();
    }

    // current finalized head is still 2
    assert_eq!(api.get_finalized_head().read().await, (json!("0xbb"), 0x02));

    let (_, head_sink2) = head_rx2.recv().await.unwrap();
    head_sink2
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x03" })).unwrap())
        .await
        .unwrap();

    let (params, tx) = block_rx2.recv().await.unwrap();
    assert_eq!(params, json!([0x03]));
    tx.send(json!("0xcc")).unwrap();

    let (_, finalized_head_sink2) = finalized_head_rx2.recv().await.unwrap();
    finalized_head_sink2
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x03" })).unwrap())
        .await
        .unwrap();

    let (params, tx) = block_rx2.recv().await.unwrap();
    assert_eq!(params, json!([0x03]));
    tx.send(json!("0xcc")).unwrap();

    head_sink2
        .send(SubscriptionMessage::from_json(&json!({ "number": "0x04" })).unwrap())
        .await
        .unwrap();

    let (params, tx) = block_rx2.recv().await.unwrap();
    assert_eq!(params, json!([0x04]));
    tx.send(json!("0xdd")).unwrap();

    // wait a bit to process tasks
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    // current head=4 and finalized_head=3
    assert_eq!(api.get_head().read().await, (json!("0xdd"), 0x04));
    assert_eq!(api.get_finalized_head().read().await, (json!("0xcc"), 0x03));

    server1.stop().unwrap();
    server2.stop().unwrap();
}
