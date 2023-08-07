use std::{str::FromStr, time::Duration};

use super::mock::*;
use super::*;

use futures::StreamExt;
use jsonrpsee::SubscriptionMessage;
use tokio::sync::{mpsc, oneshot};

#[tokio::test]
async fn basic_request() {
    let (addr, handle, mut rx, _) = dummy_server().await;

    let client = Client::new([format!("ws://{addr}")]).unwrap();

    let task = tokio::spawn(async move {
        let (params, resp_tx) = rx.recv().await.unwrap();
        assert_eq!(params.to_string(), "[1]");
        resp_tx.send(JsonValue::from_str("[1]").unwrap()).unwrap();
    });

    let result = client.request("mock_rpc", vec![1.into()]).await.unwrap();

    assert_eq!(result.to_string(), "[1]");

    handle.stop().unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn basic_subscription() {
    let (addr, handle, _, mut rx) = dummy_server().await;

    let client = Client::new([format!("ws://{addr}")]).unwrap();

    let task = tokio::spawn(async move {
        let (params, sink) = rx.recv().await.unwrap();
        assert_eq!(params.to_string(), "[123]");
        sink.send(SubscriptionMessage::from_json(&10).unwrap())
            .await
            .unwrap();
        sink.send(SubscriptionMessage::from_json(&11).unwrap())
            .await
            .unwrap();
        sink.send(SubscriptionMessage::from_json(&12).unwrap())
            .await
            .unwrap();
    });

    let result = client
        .subscribe("mock_sub", vec![123.into()], "mock_unsub")
        .await
        .unwrap();

    let result = result
        .map(|v| v.unwrap().to_string())
        .take(3)
        .collect::<Vec<_>>()
        .await;

    assert_eq!(result, ["10", "11", "12"]);

    handle.stop().unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn multiple_endpoints() {
    // create 3 dummy servers
    let (addr1, handle1, rx1, _) = dummy_server().await;
    let (addr2, handle2, rx2, _) = dummy_server().await;
    let (addr3, handle3, rx3, _) = dummy_server().await;

    let client = Client::new([
        format!("ws://{addr1}"),
        format!("ws://{addr2}"),
        format!("ws://{addr3}"),
    ])
    .unwrap();

    let handle_requests = |mut rx: mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
                           n: u32| {
        tokio::spawn(async move {
            while let Some((_, resp_tx)) = rx.recv().await {
                resp_tx.send(JsonValue::Number(n.into())).unwrap();
            }
        })
    };

    let handler1 = handle_requests(rx1, 1);
    let handler2 = handle_requests(rx2, 2);
    let handler3 = handle_requests(rx3, 3);

    let result = client.request("mock_rpc", vec![11.into()]).await.unwrap();

    assert_eq!(result.to_string(), "1");

    handle1.stop().unwrap();

    let result = client.request("mock_rpc", vec![22.into()]).await.unwrap();

    assert_eq!(result.to_string(), "2");

    client.rotate_endpoint().await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = client.request("mock_rpc", vec![33.into()]).await.unwrap();

    assert_eq!(result.to_string(), "3");

    handle3.stop().unwrap();

    let result = client.request("mock_rpc", vec![44.into()]).await.unwrap();

    assert_eq!(result.to_string(), "2");

    handle2.stop().unwrap();

    let (r1, r2, r3) = tokio::join!(handler1, handler2, handler3);
    r1.unwrap();
    r2.unwrap();
    r3.unwrap();
}

#[tokio::test]
async fn concurrent_requests() {
    let (addr, handle, mut rx, _) = dummy_server().await;

    let client = Client::new([format!("ws://{addr}")]).unwrap();

    let task = tokio::spawn(async move {
        let (_, tx1) = rx.recv().await.unwrap();
        let (_, tx2) = rx.recv().await.unwrap();
        let (_, tx3) = rx.recv().await.unwrap();

        tx1.send(JsonValue::from_str("1").unwrap()).unwrap();
        tx2.send(JsonValue::from_str("2").unwrap()).unwrap();
        tx3.send(JsonValue::from_str("3").unwrap()).unwrap();
    });

    let res1 = client.request("mock_rpc", vec![]);
    let res2 = client.request("mock_rpc", vec![]);
    let res3 = client.request("mock_rpc", vec![]);

    let res = tokio::join!(res1, res2, res3);

    assert_eq!(res.0.unwrap().to_string(), "1");
    assert_eq!(res.1.unwrap().to_string(), "2");
    assert_eq!(res.2.unwrap().to_string(), "3");

    handle.stop().unwrap();
    task.await.unwrap();
}

#[tokio::test]
#[ignore = "blocked by https://github.com/paritytech/jsonrpsee/pull/1116"]
async fn retry_requests() {
    let (addr1, handle1, mut rx1, _) = dummy_server().await;
    let (addr2, handle2, mut rx2, _) = dummy_server().await;

    let client = Client::new([format!("ws://{addr1}"), format!("ws://{addr2}")]).unwrap();

    let h1 = tokio::spawn(async move {
        let (_, tx) = rx1.recv().await.unwrap();
        // stop server after received request
        handle1.stop().unwrap();
        // handle1.stopped().await; never terminates
        // still send a valid response to avoid this become a call error
        tx.send(JsonValue::from_str("2").unwrap()).unwrap();
    });

    let h2 = tokio::spawn(async move {
        let (_, tx) = rx2.recv().await.unwrap();
        tx.send(JsonValue::from_str("1").unwrap()).unwrap();
    });

    let h3 = tokio::spawn(async move {
        let res = client.request("mock_rpc", vec![]).await.unwrap();
        assert_eq!(res.to_string(), "1");
    });

    h3.await.unwrap();
    h2.await.unwrap();
    h1.await.unwrap();

    handle2.stop().unwrap();
}
