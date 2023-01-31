use std::{net::SocketAddr, str::FromStr};

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
    tokio::join!(handler).0.unwrap();
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
        .collect::<Vec<_>>()
        .await;

    assert_eq!(result, ["10", "11", "12"]);

    handle.stop().unwrap();
    tokio::join!(handler).0.unwrap();
}
