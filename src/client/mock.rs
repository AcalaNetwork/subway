#![allow(clippy::type_complexity)] // this is mock, we don't care

use std::net::SocketAddr;

use crate::enable_logger;

use super::*;

use futures::TryFutureExt;
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

    pub fn register_method(
        &mut self,
        name: &'static str,
    ) -> mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)> {
        let (tx, rx) = mpsc::channel::<(JsonValue, oneshot::Sender<JsonValue>)>(100);
        self.module
            .register_async_method(name, move |params, _| {
                let tx = tx.clone();
                async move {
                    let (resp_tx, resp_rx) = oneshot::channel();
                    tx.send((params.parse::<JsonValue>().unwrap(), resp_tx))
                        .await
                        .unwrap();
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
    ) -> mpsc::Receiver<(JsonValue, SubscriptionSink)> {
        let (tx, rx) = mpsc::channel::<(JsonValue, SubscriptionSink)>(100);
        self.module
            .register_subscription(sub_name, method_name, unsub_name, move |params, sink, _| {
                let tx = tx.clone();
                let params = params.parse::<JsonValue>().unwrap();
                tokio::spawn(async move {
                    let sink = sink.accept().await.unwrap();
                    let _ = tx.send((params, sink)).await;
                })
                .map_err(|_| "error".into())
            })
            .unwrap();
        rx
    }

    pub async fn build(self) -> (SocketAddr, ServerHandle) {
        enable_logger(None);

        let server = ServerBuilder::default()
            .set_id_provider(RandomStringIdProvider::new(16))
            .build("0.0.0.0:0")
            .await
            .unwrap();

        let addr = server.local_addr().unwrap();
        let handle = server.start(self.module).unwrap();

        (addr, handle)
    }
}

pub async fn dummy_server() -> (
    SocketAddr,
    ServerHandle,
    mpsc::Receiver<(JsonValue, oneshot::Sender<JsonValue>)>,
    mpsc::Receiver<(JsonValue, SubscriptionSink)>,
) {
    enable_logger(None);

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
                println!("sleep {} ms", ms);
                tokio::time::sleep(std::time::Duration::from_millis(*ms)).await;
            }
            SinkTask::Send(msg) => {
                println!("send msg to sink: {}", msg);
                sink.send(SubscriptionMessage::from_json(msg).unwrap())
                    .await
                    .unwrap()
            }
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

pub async fn run_sink_tasks(sink: &SubscriptionSink, tasks: Vec<SinkTask>) {
    for task in tasks {
        task.run(sink).await
    }
}
