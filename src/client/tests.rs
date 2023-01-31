use std::{net::SocketAddr, str::FromStr, time::Duration};

use crate::enable_logger;

use super::*;

use futures::StreamExt;
use jsonrpsee::{
    server::{RandomStringIdProvider, RpcModule, ServerBuilder, ServerHandle},
    SubscriptionSink,
};
use tokio::sync::{mpsc, oneshot};

async fn dummy_server() -> (
    SocketAddr,
    ServerHandle,
    mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
) {
    enable_logger();

    let mut module = RpcModule::new(());

    let (tx, rx) = mpsc::channel::<(JsonValue, oneshot::Sender<JsonValue>)>(100);
    let (sub_tx, sub_rx) = mpsc::channel::<(JsonValue, SubscriptionSink)>(100);

    module
        .register_async_method("mock_rpc", move |params, _| {
            let tx = tx.clone();
            async move {
                let (resp_tx, resp_rx) = oneshot::channel();
                tx.send((params.parse::<JsonValue>().unwrap(), resp_tx))
                    .await
                    .unwrap();
                let res = resp_rx.await;
                Ok::<JsonValue, jsonrpsee::core::Error>(res.unwrap())
            }
        })
        .unwrap();

    module
        .register_subscription("mock_sub", "sub", "mock_unsub", move |params, sink, _| {
            let params = params.parse::<JsonValue>().unwrap();
            let sub_tx = sub_tx.clone();
            tokio::spawn(async move {
                sub_tx.send((params, sink)).await.unwrap();
            });
            Ok(())
        })
        .unwrap();

    let server = ServerBuilder::default()
        .set_id_provider(RandomStringIdProvider::new(16))
        .build("0.0.0.0:0")
        .await
        .unwrap();

    let addr = server.local_addr().unwrap();
    let handle = server.start(module).unwrap();

    (addr, handle, rx, sub_rx)
}

#[tokio::test]
async fn basic_request() {
    let (addr, handle, mut rx, _) = dummy_server().await;

    let client = Client::new(&[format!("ws://{addr}")]).await.unwrap();

    let handler = tokio::spawn(async move {
        let (params, resp_tx) = rx.recv().await.unwrap();
        assert_eq!(params.to_string(), "[1]");
        resp_tx.send(JsonValue::from_str("[1]").unwrap()).unwrap();
    });

    let result = client
        .request("mock_rpc", Params::new(Some("[1]")))
        .await
        .unwrap();

    assert_eq!(result.to_string(), "[1]");

    handle.stop().unwrap();
    let _ = tokio::join!(handler);
}

#[tokio::test]
async fn basic_subscription() {
    let (addr, handle, _, mut rx) = dummy_server().await;

    let client = Client::new(&[format!("ws://{addr}")]).await.unwrap();

    let handler = tokio::spawn(async move {
        let (params, mut sink) = rx.recv().await.unwrap();
        assert_eq!(params.to_string(), "[123]");
        sink.send(&JsonValue::from_str("10").unwrap()).unwrap();
        sink.send(&JsonValue::from_str("11").unwrap()).unwrap();
        sink.send(&JsonValue::from_str("12").unwrap()).unwrap();
    });

    let result = client
        .subscribe("mock_sub", Params::new(Some("[123]")), "mock_unsub")
        .await
        .unwrap();

    let result = result
        .map(|v| v.unwrap().to_string())
        .take(3)
        .collect::<Vec<_>>()
        .await;

    assert_eq!(result, ["10", "11", "12"]);

    handle.stop().unwrap();
    let _ = tokio::join!(handler);
}

#[tokio::test]
async fn multiple_endpoints() {
    // create 3 dummy servers
    let (addr1, handle1, rx1, _) = dummy_server().await;
    let (addr2, handle2, rx2, _) = dummy_server().await;
    let (addr3, handle3, rx3, _) = dummy_server().await;

    let client = Client::new(&[
        format!("ws://{addr1}"),
        format!("ws://{addr2}"),
        format!("ws://{addr3}"),
    ])
    .await
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

    let result = client
        .request("mock_rpc", Params::new(Some("[11]")))
        .await
        .unwrap();

    assert_eq!(result.to_string(), "1");

    handle1.stop().unwrap();

    let result = client
        .request("mock_rpc", Params::new(Some("[22]")))
        .await
        .unwrap();

    assert_eq!(result.to_string(), "2");

    client.rotate_endpoint().await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let result = client
        .request("mock_rpc", Params::new(Some("[33]")))
        .await
        .unwrap();

    assert_eq!(result.to_string(), "3");

    handle3.stop().unwrap();

    let result = client
        .request("mock_rpc", Params::new(Some("[44]")))
        .await
        .unwrap();

    assert_eq!(result.to_string(), "2");

    handle2.stop().unwrap();

    let _ = tokio::join!(handler1, handler2, handler3);
}

#[tokio::test]
async fn concurrent_requests() {
    let (addr, handle, mut rx, _) = dummy_server().await;

    let client = Client::new(&[format!("ws://{addr}")]).await.unwrap();

    let handler = tokio::spawn(async move {
        let (_, tx1) = rx.recv().await.unwrap();
        let (_, tx2) = rx.recv().await.unwrap();
        let (_, tx3) = rx.recv().await.unwrap();

        tx1.send(JsonValue::from_str("1").unwrap()).unwrap();
        tx2.send(JsonValue::from_str("2").unwrap()).unwrap();
        tx3.send(JsonValue::from_str("3").unwrap()).unwrap();
    });

    let res1 = client.request("mock_rpc", Params::new(Some("[]")));
    let res2 = client.request("mock_rpc", Params::new(Some("[]")));
    let res3 = client.request("mock_rpc", Params::new(Some("[]")));

    let res = tokio::join!(res1, res2, res3);

    assert_eq!(res.0.unwrap().to_string(), "1");
    assert_eq!(res.1.unwrap().to_string(), "2");
    assert_eq!(res.2.unwrap().to_string(), "3");

    handle.stop().unwrap();
    let _ = tokio::join!(handler);
}
