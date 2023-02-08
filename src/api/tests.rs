use jsonrpsee::{server::ServerHandle, SubscriptionSink};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use super::*;

use crate::client::mock::TestServerBuilder;

async fn create_client() -> (
    Api,
    ServerHandle,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
    mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
) {
    let mut builder = TestServerBuilder::new();

    let head_rx = builder.register_subscription(
        "chain_subscribeNewHeads",
        "chain_newHead",
        "chain_unsubscribeNewHeads",
    );

    let finalized_head_rx = builder.register_subscription(
        "chain_subscribeFinalizedHeads",
        "chain_finalizedHead",
        "chain_unsubscribeFinalizedHeads",
    );

    let block_hash_rx = builder.register_method("chain_getBlockHash");

    let (addr, server) = builder.build().await;

    let client = Client::new(&[format!("ws://{addr}")]).await.unwrap();
    let api = Api::new(Arc::new(client));

    (api, server, head_rx, finalized_head_rx, block_hash_rx)
}

#[tokio::test]
async fn get_head_finalized_head() {
    let (api, server, mut head_rx, mut finalized_head_rx, mut block_rx) = create_client().await;

    let head = api.get_head();
    let finalized_head = api.get_finalized_head();

    // access value before subscription is established

    let h1 = tokio::spawn(async move {
        assert_eq!(head.read().await, (json!("0xabcd"), 0x1234));
        // should be able to read it multiple times
        assert_eq!(head.read().await, (json!("0xabcd"), 0x1234));
    });

    let (_, mut head_sink) = head_rx.recv().await.unwrap();
    head_sink.send(&json!({ "number": "0x1234" })).unwrap();

    {
        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x1234]));
        tx.send(json!("0xabcd")).unwrap();
    }

    let (_, mut finalized_head_sink) = finalized_head_rx.recv().await.unwrap();
    finalized_head_sink
        .send(&json!({ "number": "0x4321" }))
        .unwrap();

    {
        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x4321]));
        tx.send(json!("0xdcba")).unwrap();
    }

    // read after subscription is established

    let h2 = tokio::spawn(async move {
        let val = finalized_head.read().await;
        assert_eq!(val, (json!("0xdcba"), 0x4321));
    });

    // new head

    head_sink.send(&json!({ "number": "0x1122" })).unwrap();

    {
        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x1122]));
        tx.send(json!("0xaabb")).unwrap();
    }

    let finalized_head = api.get_finalized_head();
    // still old value
    assert_eq!(finalized_head.read().await, (json!("0xdcba"), 0x4321));

    // wait a bit for the value to be updated
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    let head = api.get_head();
    assert_eq!(head.read().await, (json!("0xaabb"), 0x1122));

    // new finalized head
    finalized_head_sink
        .send(&json!({ "number": "0x2233" }))
        .unwrap();
    finalized_head_sink
        .send(&json!({ "number": "0x3344" }))
        .unwrap();

    {
        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x2233]));
        tx.send(json!("0xbbcc")).unwrap();

        let (params, tx) = block_rx.recv().await.unwrap();
        assert_eq!(params, json!([0x3344]));
        tx.send(json!("0xccdd")).unwrap();
    }

    // wait a bit for the value to be updated
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    assert_eq!(finalized_head.read().await, (json!("0xccdd"), 0x3344));

    h1.await.unwrap();
    h2.await.unwrap();
    server.stop().unwrap();
}
