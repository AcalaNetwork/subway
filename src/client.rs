use std::sync::atomic::AtomicUsize;

use jsonrpsee::{
    core::{
        client::{ClientT, Subscription, SubscriptionClientT},
        Error, JsonValue,
    },
    types::{error::CallError, Params},
    ws_client::WsClientBuilder,
};

use crate::config::Config;

pub struct Client {
    sender: tokio::sync::mpsc::Sender<Message>,
}

#[derive(Debug)]
enum Message {
    Request {
        method: String,
        params: Vec<JsonValue>,
        response: tokio::sync::oneshot::Sender<Result<JsonValue, Error>>,
    },
    Subscribe {
        subscribe: String,
        params: Vec<JsonValue>,
        unsubscribe: String,
        response: tokio::sync::oneshot::Sender<Result<Subscription<JsonValue>, Error>>,
    },
}

impl Client {
    pub async fn new(endpoints: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Self, String> {
        let endpoints: Vec<_> = endpoints
            .into_iter()
            .map(|e| e.as_ref().to_string())
            .collect();

        if endpoints.is_empty() {
            return Err("No endpoints provided".into());
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(100);

        tokio::spawn(async move {
            let current_endpoint = AtomicUsize::new(0);

            let build_ws = || async {
                let build = || {
                    let current_endpoint = current_endpoint.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let url = &endpoints[current_endpoint % endpoints.len()];
                    WsClientBuilder::default()
                        .request_timeout(std::time::Duration::from_secs(60))
                        .connection_timeout(std::time::Duration::from_secs(30))
                        .max_notifs_per_subscription(1024)
                        .build(url)
                };

                loop {
                    match build().await {
                        Ok(ws) => break ws,
                        Err(e) => {
                            log::debug!("Unable to connect to endpoint: {}", e);
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                    }
                }
            };

            let mut ws = build_ws().await;

            loop {
                tokio::select! {
                    _ = ws.on_disconnect() => {
                        log::debug!("Disconnected from endpoint");
                        ws = build_ws().await;
                    }
                    message = rx.recv() => {
                        match message {
                            Some(Message::Request {
                                method,
                                params,
                                response,
                            }) => {
                                let res = ws.request(&method, params).await;
                                if response.send(res).is_err() {
                                    log::warn!("Unable to send response");
                                }
                            }
                            Some(Message::Subscribe {
                                subscribe,
                                params,
                                unsubscribe,
                                response,
                            }) => {
                                let res = ws.subscribe(&subscribe, params, &unsubscribe).await;
                                if response.send(res).is_err() {
                                    log::warn!("Unable to send response");
                                }
                            }
                            None => {
                                log::debug!("Client dropped");
                                break;
                            }
                        }
                    },
                };
            }
        });

        Ok(Self { sender: tx })
    }

    pub async fn request(&self, method: &str, params: Params<'_>) -> Result<JsonValue, Error> {
        let params: Vec<JsonValue> = params.parse()?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(Message::Request {
                method: method.into(),
                params,
                response: tx,
            })
            .await
            .map_err(|e| CallError::Failed(e.into()))?;

        rx.await.map_err(|e| CallError::Failed(e.into()))?
    }

    pub async fn subscribe(
        &self,
        subscribe: &str,
        params: Params<'_>,
        unsubscribe: &str,
    ) -> Result<Subscription<JsonValue>, Error> {
        let params: Vec<JsonValue> = params.parse()?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(Message::Subscribe {
                subscribe: subscribe.into(),
                params,
                unsubscribe: unsubscribe.into(),
                response: tx,
            })
            .await
            .map_err(|e| CallError::Failed(e.into()))?;

        rx.await.map_err(|e| CallError::Failed(e.into()))?
    }
}

pub async fn create_client(config: &Config) -> anyhow::Result<Client> {
    let endpoints = config.endpoints.clone();

    Client::new(endpoints).await.map_err(anyhow::Error::msg)
}
