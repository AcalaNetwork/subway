#![allow(clippy::type_complexity)] // this is mock, we don't care

use std::net::SocketAddr;

use crate::enable_logger;

use super::*;

use jsonrpsee::{
    server::{RandomStringIdProvider, RpcModule, ServerBuilder, ServerHandle},
    SubscriptionSink,
};
use tokio::sync::{mpsc, oneshot, Mutex};

pub struct TestServerBuilder {
    methods: Vec<(
        &'static str,
        Box<dyn FnOnce(mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>)>,
    )>,
    subscriptions: Vec<(
        &'static str,
        &'static str,
        &'static str,
        Box<dyn FnOnce(mpsc::Receiver<(JsonValue, SubscriptionSink)>)>,
    )>,
}

impl TestServerBuilder {
    pub fn new() -> Self {
        Self {
            methods: vec![],
            subscriptions: vec![],
        }
    }

    pub fn register_method(
        mut self,
        name: &'static str,
        f: impl Fn(mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>) + 'static,
    ) -> Self {
        self.methods.push((name, Box::new(f)));
        self
    }

    pub fn register_subscription(
        mut self,
        name: &'static str,
        sub_name: &'static str,
        unsub_name: &'static str,
        f: impl Fn(mpsc::Receiver<(JsonValue, SubscriptionSink)>) + 'static,
    ) -> Self {
        self.subscriptions
            .push((name, sub_name, unsub_name, Box::new(f)));
        self
    }

    pub async fn build(self) -> (SocketAddr, ServerHandle) {
        enable_logger();

        let mut module = RpcModule::new(());

        for (name, f) in self.methods {
            let (tx, rx) = mpsc::channel::<(JsonValue, oneshot::Sender<JsonValue>)>(100);
            f(rx);
            module
                .register_async_method(name, move |params, _| {
                    let tx = tx.clone();
                    async move {
                        let (resp_tx, resp_rx) = oneshot::channel();
                        tx.send((params.parse::<JsonValue>().unwrap(), resp_tx))
                            .await
                            .unwrap();
                        let res = resp_rx.await;
                        res.map_err(|e| -> Error { CallError::Failed(e.into()).into() })
                    }
                })
                .unwrap();
        }

        for (sub_name, method_name, unsub_name, f) in self.subscriptions {
            let (tx, rx) = mpsc::channel::<(JsonValue, SubscriptionSink)>(100);
            f(rx);
            module
                .register_subscription(sub_name, method_name, unsub_name, move |params, sink, _| {
                    let tx = tx.clone();
                    let params = params.parse::<JsonValue>().unwrap();
                    tokio::spawn(async move {
                        tx.send((params, sink)).await.unwrap();
                    });
                    Ok(())
                })
                .unwrap();
        }

        let server = ServerBuilder::default()
            .set_id_provider(RandomStringIdProvider::new(16))
            .build("0.0.0.0:0")
            .await
            .unwrap();

        let addr = server.local_addr().unwrap();
        let handle = server.start(module).unwrap();

        (addr, handle)
    }
}

pub async fn dummy_server() -> (
    SocketAddr,
    ServerHandle,
    mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
) {
    enable_logger();

    let rx = Arc::new(Mutex::<
        Option<mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>>,
    >::new(None));
    let sub_rx =
        Arc::new(Mutex::<Option<mpsc::Receiver<(JsonValue, SubscriptionSink)>>>::new(None));

    let rx_clone = rx.clone();
    let sub_rx_clone = sub_rx.clone();

    let (addr, handle) = TestServerBuilder::new()
        .register_method("mock_rpc", move |rx2| {
            let rx_clone = rx_clone.clone();
            tokio::spawn(async move {
                (*rx_clone.lock().await) = Some(rx2);
            });
        })
        .register_subscription("mock_sub", "mock", "mock_unsub", move |rx| {
            let sub_rx_clone = sub_rx_clone.clone();
            tokio::spawn(async move {
                (*sub_rx_clone.lock().await) = Some(rx);
            });
        })
        .build()
        .await;

    // TODO: actually wait for the data to be received
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;

    let mut guard_rx = rx.lock().await;
    let rx = guard_rx.take().unwrap();
    drop(guard_rx);

    let mut guard_sub_rx = sub_rx.lock().await;
    let sub_rx = guard_sub_rx.take().unwrap();
    drop(guard_sub_rx);

    (addr, handle, rx, sub_rx)
}
