use std::{
    sync::{
        atomic::{AtomicU32, AtomicUsize},
        Arc,
    },
    time::Duration,
};

use anyhow::anyhow;
use async_trait::async_trait;
use futures::TryFutureExt;
use jsonrpsee::{
    core::{
        client::{ClientT, Subscription, SubscriptionClientT},
        Error, JsonValue,
    },
    types::ErrorObjectOwned,
    ws_client::{WsClient, WsClientBuilder},
};
use opentelemetry::trace::FutureExt;
use rand::{seq::SliceRandom, thread_rng};
use serde::Deserialize;
use tokio::sync::Notify;

use crate::{
    extension::Extension,
    middleware::ExtensionRegistry,
    utils::{self, errors},
};

#[cfg(test)]
pub mod mock;
#[cfg(test)]
mod tests;

const TRACER: utils::telemetry::Tracer = utils::telemetry::Tracer::new("client");

pub struct Client {
    sender: tokio::sync::mpsc::Sender<Message>,
    rotation_notify: Arc<Notify>,
}

#[derive(Deserialize, Debug)]
pub struct ClientConfig {
    pub endpoints: Vec<String>,
    #[serde(default = "bool_true")]
    pub shuffle_endpoints: bool,
}

pub fn bool_true() -> bool {
    true
}

#[derive(Debug)]
enum Message {
    Request {
        method: String,
        params: Vec<JsonValue>,
        response: tokio::sync::oneshot::Sender<Result<JsonValue, Error>>,
        retries: u32,
    },
    Subscribe {
        subscribe: String,
        params: Vec<JsonValue>,
        unsubscribe: String,
        response: tokio::sync::oneshot::Sender<Result<Subscription<JsonValue>, Error>>,
        retries: u32,
    },
    RotateEndpoint,
}

#[async_trait]
impl Extension for Client {
    type Config = ClientConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        if config.shuffle_endpoints {
            let mut endpoints = config.endpoints.clone();
            endpoints.shuffle(&mut thread_rng());
            Ok(Self::new(endpoints)?)
        } else {
            Ok(Self::new(config.endpoints.clone())?)
        }
    }
}

impl Client {
    pub fn new(endpoints: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Self, anyhow::Error> {
        let endpoints: Vec<_> = endpoints.into_iter().map(|e| e.as_ref().to_string()).collect();

        if endpoints.is_empty() {
            return Err(anyhow!("No endpoints provided"));
        }

        tracing::debug!("New client with endpoints: {:?}", endpoints);

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(100);

        let tx2 = tx.clone();

        let rotation_notify = Arc::new(Notify::new());
        let rotating = rotation_notify.clone();

        tokio::spawn(async move {
            let tx = tx2;

            let connect_backoff_counter = Arc::new(AtomicU32::new(0));
            let request_backoff_counter = Arc::new(AtomicU32::new(0));

            let current_endpoint = AtomicUsize::new(0);

            let connect_backoff_counter2 = connect_backoff_counter.clone();
            let build_ws = || async {
                let build = || {
                    let current_endpoint = current_endpoint.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let url = &endpoints[current_endpoint % endpoints.len()];

                    tracing::info!("Connecting to endpoint: {}", url);

                    // TODO: make those configurable
                    WsClientBuilder::default()
                        .request_timeout(std::time::Duration::from_secs(30))
                        .connection_timeout(std::time::Duration::from_secs(30))
                        .max_buffer_capacity_per_subscription(2048)
                        .max_concurrent_requests(2048)
                        .max_response_size(20 * 1024 * 1024)
                        .build(url)
                        .map_err(|e| (e, url.to_string()))
                };

                loop {
                    match build().await {
                        Ok(ws) => {
                            let ws = Arc::new(ws);
                            tracing::info!("Endpoint connected");
                            connect_backoff_counter2.store(0, std::sync::atomic::Ordering::Relaxed);
                            break ws;
                        }
                        Err((e, url)) => {
                            tracing::warn!("Unable to connect to endpoint: '{url}' error: {e}");
                            tokio::time::sleep(get_backoff_time(&connect_backoff_counter2)).await;
                        }
                    }
                }
            };

            let mut ws = build_ws().await;

            let handle_message = |message: Message, ws: Arc<WsClient>| {
                let tx = tx.clone();
                let request_backoff_counter = request_backoff_counter.clone();

                tokio::spawn(async move {
                    match message {
                        Message::Request {
                            method,
                            params,
                            response,
                            retries,
                        } => {
                            // make sure it's still connected
                            if response.is_closed() {
                                return;
                            }

                            let result = ws.request(&method, params.clone()).await;
                            match result {
                                result @ Ok(_) => {
                                    request_backoff_counter.store(0, std::sync::atomic::Ordering::Relaxed);
                                    // make sure it's still connected
                                    if response.is_closed() {
                                        return;
                                    }
                                    let _ = response.send(result);
                                }
                                Err(err) => {
                                    tracing::debug!("Request failed: {:?}", err);
                                    match err {
                                        Error::RequestTimeout
                                        | Error::Transport(_)
                                        | Error::RestartNeeded(_)
                                        | Error::MaxSlotsExceeded => {
                                            tokio::time::sleep(get_backoff_time(&request_backoff_counter)).await;

                                            // make sure it's still connected
                                            if response.is_closed() {
                                                return;
                                            }

                                            // make sure we still have retries left
                                            if retries == 0 {
                                                let _ = response.send(Err(Error::RequestTimeout));
                                                return;
                                            }

                                            if matches!(err, Error::RequestTimeout) {
                                                tx.send(Message::RotateEndpoint)
                                                    .await
                                                    .expect("Failed to send rotate message");
                                            }

                                            tx.send(Message::Request {
                                                method,
                                                params,
                                                response,
                                                retries: retries - 1,
                                            })
                                            .await
                                            .expect("Failed to send request message");
                                        }
                                        err => {
                                            // make sure it's still connected
                                            if response.is_closed() {
                                                return;
                                            }
                                            // not something we can handle, send it back to the caller
                                            let _ = response.send(Err(err));
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
                            retries,
                        } => {
                            let result = ws.subscribe(&subscribe, params.clone(), &unsubscribe).await;
                            match result {
                                result @ Ok(_) => {
                                    request_backoff_counter.store(0, std::sync::atomic::Ordering::Relaxed);
                                    // make sure it's still connected
                                    if response.is_closed() {
                                        return;
                                    }
                                    let _ = response.send(result);
                                }
                                Err(err) => {
                                    tracing::debug!("Subscribe failed: {:?}", err);
                                    match err {
                                        Error::RequestTimeout
                                        | Error::Transport(_)
                                        | Error::RestartNeeded(_)
                                        | Error::MaxSlotsExceeded => {
                                            tokio::time::sleep(get_backoff_time(&request_backoff_counter)).await;

                                            // make sure it's still connected
                                            if response.is_closed() {
                                                return;
                                            }

                                            // make sure we still have retries left
                                            if retries == 0 {
                                                let _ = response.send(Err(Error::RequestTimeout));
                                                return;
                                            }

                                            if matches!(err, Error::RequestTimeout) {
                                                tx.send(Message::RotateEndpoint)
                                                    .await
                                                    .expect("Failed to send rotate message");
                                            }

                                            tx.send(Message::Subscribe {
                                                subscribe,
                                                params,
                                                unsubscribe,
                                                response,
                                                retries: retries - 1,
                                            })
                                            .await
                                            .expect("Failed to send subscribe message")
                                        }
                                        err => {
                                            // make sure it's still connected
                                            if response.is_closed() {
                                                return;
                                            }
                                            // not something we can handle, send it back to the caller
                                            let _ = response.send(Err(err));
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
                    _ = ws.on_disconnect() => {
                        tracing::info!("Endpoint disconnected");
                        tokio::time::sleep(get_backoff_time(&connect_backoff_counter)).await;
                        ws = build_ws().await;
                    }
                    message = rx.recv() => {
                        tracing::trace!("Received message {message:?}");
                        match message {
                            Some(Message::RotateEndpoint) => {
                                rotating.notify_waiters();
                                tracing::info!("Rotate endpoint");
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

        Ok(Self {
            sender: tx,
            rotation_notify,
        })
    }

    pub async fn request(&self, method: &str, params: Vec<JsonValue>) -> Result<JsonValue, ErrorObjectOwned> {
        let cx = TRACER.context(method.to_string());
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(Message::Request {
                method: method.into(),
                params,
                response: tx,
                retries: 3,
            })
            .await
            .map_err(errors::internal_error)?;

        rx.with_context(cx)
            .await
            .map_err(errors::internal_error)?
            .map_err(errors::map_error)
    }

    pub async fn subscribe(
        &self,
        subscribe: &str,
        params: Vec<JsonValue>,
        unsubscribe: &str,
    ) -> Result<Subscription<JsonValue>, Error> {
        let cx = TRACER.context(subscribe.to_string());

        let (tx, rx) = tokio::sync::oneshot::channel();
        self.sender
            .send(Message::Subscribe {
                subscribe: subscribe.into(),
                params,
                unsubscribe: unsubscribe.into(),
                response: tx,
                retries: 3,
            })
            .await
            .map_err(errors::failed)?;

        rx.with_context(cx).await.map_err(errors::failed)?
    }

    pub async fn rotate_endpoint(&self) {
        self.sender
            .send(Message::RotateEndpoint)
            .await
            .expect("Failed to rotate endpoint");
    }

    /// Returns a future that resolves when the endpoint is rotated.
    pub async fn on_rotation(&self) {
        self.rotation_notify.notified().await
    }
}

fn get_backoff_time(counter: &Arc<AtomicU32>) -> Duration {
    let min_time = 100u64;
    let step = 100u64;
    let max_count = 10u32;

    let backoff_count = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let backoff_count = backoff_count.min(max_count) as u64;
    let backoff_time = backoff_count * backoff_count * step;

    Duration::from_millis(backoff_time + min_time)
}

#[test]
fn test_get_backoff_time() {
    let counter = Arc::new(AtomicU32::new(0));

    let mut times = Vec::new();

    for _ in 0..12 {
        times.push(get_backoff_time(&counter));
    }

    let times = times.into_iter().map(|t| t.as_millis()).collect::<Vec<_>>();

    assert_eq!(
        times,
        vec![100, 200, 500, 1000, 1700, 2600, 3700, 5000, 6500, 8200, 10100, 10100]
    );
}
