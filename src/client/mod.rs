use std::sync::{atomic::AtomicUsize, Arc};

use futures::TryFutureExt;
use jsonrpsee::{
    core::{
        client::{ClientT, Subscription, SubscriptionClientT},
        Error, JsonValue,
    },
    types::error::CallError,
    ws_client::{WsClient, WsClientBuilder},
};

#[cfg(test)]
pub mod mock;
#[cfg(test)]
mod tests;

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
    RotateEndpoint,
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

        tracing::debug!("New client with endpoints: {:?}", endpoints);

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(100);

        let tx2 = tx.clone();

        let (disconnect_tx, mut disconnect_rx) = tokio::sync::mpsc::channel::<()>(10);

        tokio::spawn(async move {
            let tx = tx2;

            let current_endpoint = AtomicUsize::new(0);

            let build_ws = || async {
                let build = || {
                    let current_endpoint =
                        current_endpoint.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let url = &endpoints[current_endpoint % endpoints.len()];

                    tracing::info!("Connecting to endpoint: {}", url);

                    WsClientBuilder::default()
                        .request_timeout(std::time::Duration::from_secs(30))
                        .connection_timeout(std::time::Duration::from_secs(30))
                        .max_buffer_capacity_per_subscription(1024)
                        .build(url)
                        .map_err(|e| (e, url.to_string()))
                };

                let disconnect_tx = disconnect_tx.clone();

                loop {
                    match build().await {
                        Ok(ws) => {
                            let ws = Arc::new(ws);
                            let ws2 = ws.clone();

                            tokio::spawn(async move {
                                ws2.on_disconnect().await;
                                if let Err(e) = disconnect_tx.send(()).await {
                                    tracing::warn!("Unable to send disconnect: {}", e);
                                }
                            });
                            break ws;
                        }
                        Err((e, url)) => {
                            tracing::warn!("Unable to connect to endpoint: '{url}' error: {e}");
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                    }
                }
            };

            let mut ws = build_ws().await;

            let handle_message = |message: Message, ws: Arc<WsClient>| {
                let tx = tx.clone();

                tokio::spawn(async move {
                    match message {
                        Message::Request {
                            method,
                            params,
                            response,
                        } => {
                            let result = ws.request(&method, params.clone()).await;
                            match result {
                                result @ Ok(_) => {
                                    if let Err(e) = response.send(result) {
                                        tracing::warn!("Failed to send response: {:?}", e);
                                    }
                                }
                                Err(err) => {
                                    tracing::debug!("Request failed: {:?}", err);
                                    match err {
                                        Error::RequestTimeout => {
                                            if let Err(e) = tx.send(Message::RotateEndpoint).await {
                                                tracing::warn!(
                                                    "Failed to send rotate message: {:?}",
                                                    e
                                                );
                                            }
                                            if let Err(e) = tx
                                                .send(Message::Request {
                                                    method,
                                                    params,
                                                    response,
                                                })
                                                .await
                                            {
                                                tracing::warn!(
                                                    "Failed to send request message: {:?}",
                                                    e
                                                );
                                            }
                                        }
                                        Error::Transport(_) | Error::RestartNeeded(_) => {
                                            if let Err(e) = tx
                                                .send(Message::Request {
                                                    method,
                                                    params,
                                                    response,
                                                })
                                                .await
                                            {
                                                tracing::warn!(
                                                    "Failed to send request message: {:?}",
                                                    e
                                                );
                                            }
                                        }
                                        err => {
                                            // not something we can handle, send it back to the caller
                                            if let Err(e) = response.send(Err(err)) {
                                                tracing::warn!("Failed to send response: {:?}", e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Message::Subscribe {
                            subscribe,
                            params,
                            unsubscribe,
                            response,
                        } => {
                            let result =
                                ws.subscribe(&subscribe, params.clone(), &unsubscribe).await;
                            match result {
                                result @ Ok(_) => {
                                    if let Err(e) = response.send(result) {
                                        tracing::warn!("Failed to send response: {:?}", e);
                                    }
                                }
                                Err(err) => {
                                    tracing::debug!("Subscribe failed: {:?}", err);
                                    match err {
                                        Error::RequestTimeout => {
                                            if let Err(e) = tx.send(Message::RotateEndpoint).await {
                                                tracing::warn!(
                                                    "Failed to send rotate message: {:?}",
                                                    e
                                                );
                                            }
                                            if let Err(e) = tx
                                                .send(Message::Subscribe {
                                                    subscribe,
                                                    params,
                                                    unsubscribe,
                                                    response,
                                                })
                                                .await
                                            {
                                                tracing::warn!(
                                                    "Failed to send subscribe message: {:?}",
                                                    e
                                                );
                                            }
                                        }
                                        Error::Transport(_) | Error::RestartNeeded(_) => {
                                            if let Err(e) = tx
                                                .send(Message::Subscribe {
                                                    subscribe,
                                                    params,
                                                    unsubscribe,
                                                    response,
                                                })
                                                .await
                                            {
                                                tracing::warn!(
                                                    "Failed to send subscribe message: {:?}",
                                                    e
                                                );
                                            }
                                        }
                                        err => {
                                            // not something we can handle, send it back to the caller
                                            if let Err(e) = response.send(Err(err)) {
                                                tracing::warn!("Failed to send response: {:?}", e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Message::RotateEndpoint => {
                            unreachable!()
                        }
                    }
                });
            };

            loop {
                tokio::select! {
                    _ = disconnect_rx.recv() => {
                        tracing::debug!("Disconnected from endpoint");
                        ws = build_ws().await;
                    }
                    message = rx.recv() => {
                        tracing::trace!("Received message {message:?}");
                        match message {
                            Some(Message::RotateEndpoint) => {
                                ws = build_ws().await;
                            }
                            Some(message) => handle_message(message, ws.clone()),
                            None => {
                                tracing::debug!("Client dropped");
                                break;
                            }
                        }
                    },
                };
            }
        });

        Ok(Self { sender: tx })
    }

    pub async fn request(&self, method: &str, params: Vec<JsonValue>) -> Result<JsonValue, Error> {
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
        params: Vec<JsonValue>,
        unsubscribe: &str,
    ) -> Result<Subscription<JsonValue>, Error> {
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

    pub async fn rotate_endpoint(&self) -> Result<(), ()> {
        self.sender
            .send(Message::RotateEndpoint)
            .await
            .map_err(|_| ())
    }
}
