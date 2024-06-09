#![allow(clippy::type_complexity)] // this is mock, we don't care

use std::net::SocketAddr;

use crate::logger::enable_logger;

use super::*;

use futures::TryFutureExt;
use jsonrpsee::types::ErrorObject;
use jsonrpsee::{
    server::{RandomStringIdProvider, RpcModule, ServerBuilder, ServerHandle},
    SubscriptionMessage, SubscriptionSink,
};
use tokio::sync::{mpsc, oneshot};

pub struct TestServerBuilder {
    module: RpcModule<()>,
}

impl Default for TestServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TestServerBuilder {
    pub fn new() -> Self {
        Self {
            module: RpcModule::new(()),
        }
    }

    pub fn register_method(&mut self, name: &'static str) -> mpsc::Receiver<MockRequest> {
        let (tx, rx) = mpsc::channel::<MockRequest>(100);
        self.module
            .register_async_method(name, move |params, _, _| {
                let tx = tx.clone();
                let params = params.parse::<JsonValue>().unwrap();
                async move {
                    let (resp_tx, resp_rx) = oneshot::channel();
                    tx.send(MockRequest { params, resp_tx }).await.unwrap();
                    let res = resp_rx.await;
                    res.map_err(errors::failed)
                }
            })
            .unwrap();
        rx
    }

    pub fn register_subscription(
        &mut self,
        sub_name: &'static str,
        method_name: &'static str,
        unsub_name: &'static str,
    ) -> mpsc::Receiver<MockSubscription> {
        let (tx, rx) = mpsc::channel::<MockSubscription>(100);
        self.module
            .register_subscription(sub_name, method_name, unsub_name, move |params, sink, _, _| {
                let tx = tx.clone();
                let params = params.parse::<JsonValue>().unwrap();
                tokio::spawn(async move {
                    let sink = sink.accept().await.unwrap();
                    let _ = tx.send(MockSubscription { params, sink }).await;
                })
                .map_err(|_| "error".into())
            })
            .unwrap();
        rx
    }

    pub fn register_error_subscription(
        &mut self,
        sub_name: &'static str,
        method_name: &'static str,
        unsub_name: &'static str,
    ) {
        self.module
            .register_subscription(sub_name, method_name, unsub_name, move |_, sink, _, _| async {
                sink.reject(errors::map_error(Error::Call(ErrorObject::owned(
                    1010,
                    "Invalid Transaction",
                    Some("Inability to pay some fees (e.g. account balance too low)"),
                ))))
                .await;
                Ok(())
            })
            .unwrap();
    }

    pub async fn build(self) -> (SocketAddr, ServerHandle) {
        enable_logger();

        let server = ServerBuilder::default()
            .set_id_provider(RandomStringIdProvider::new(16))
            .build("0.0.0.0:0")
            .await
            .unwrap();

        let addr = server.local_addr().unwrap();
        let handle = server.start(self.module);

        (addr, handle)
    }
}

pub struct MockRequest {
    pub params: JsonValue,
    pub resp_tx: oneshot::Sender<JsonValue>,
}

impl MockRequest {
    pub fn respond(self, resp: JsonValue) {
        self.resp_tx.send(resp).unwrap();
    }
}

pub struct MockSubscription {
    pub params: JsonValue,
    pub sink: SubscriptionSink,
}

impl MockSubscription {
    pub async fn send(&self, msg: JsonValue) {
        self.sink
            .send(SubscriptionMessage::from_json(&msg).unwrap())
            .await
            .unwrap();
    }

    pub async fn run_sink_tasks(&self, tasks: Vec<SinkTask>) {
        for task in tasks {
            task.run(&self.sink).await
        }
    }
}

pub async fn dummy_server() -> (
    SocketAddr,
    ServerHandle,
    mpsc::Receiver<MockRequest>,
    mpsc::Receiver<MockSubscription>,
) {
    enable_logger();

    let mut builder = TestServerBuilder::new();

    let rx = builder.register_method("mock_rpc");
    let sub_rx = builder.register_subscription("mock_sub", "mock", "mock_unsub");

    let (addr, handle) = builder.build().await;

    (addr, handle, rx, sub_rx)
}

pub enum SinkTask {
    Sleep(u64),
    Send(JsonValue),
    SinkClosed(Option<u64>),
}

impl SinkTask {
    async fn run(&self, sink: &SubscriptionSink) {
        match self {
            SinkTask::Sleep(ms) => {
                tokio::time::sleep(Duration::from_millis(*ms)).await;
            }
            SinkTask::Send(msg) => sink.send(SubscriptionMessage::from_json(msg).unwrap()).await.unwrap(),
            SinkTask::SinkClosed(duration) => {
                let begin = std::time::Instant::now();
                sink.closed().await;
                if let Some(duration) = *duration {
                    assert_eq!(begin.elapsed().as_secs(), duration);
                }
            }
        }
    }
}
