use std::{
    sync::{atomic::AtomicU32, Arc},
    time::Duration,
};

use anyhow::anyhow;
use async_trait::async_trait;
use jsonrpsee::core::{client::Subscription, Error, JsonValue};
use opentelemetry::trace::FutureExt;
use rand::{seq::SliceRandom, thread_rng};
use serde::Deserialize;
use tokio::sync::Notify;

use super::ExtensionRegistry;
use crate::{
    extensions::Extension,
    middlewares::CallResult,
    utils::{self, errors},
};

mod endpoint;
mod health;
use endpoint::Endpoint;

#[cfg(test)]
pub mod mock;
#[cfg(test)]
mod tests;

const TRACER: utils::telemetry::Tracer = utils::telemetry::Tracer::new("client");

pub struct Client {
    sender: tokio::sync::mpsc::Sender<Message>,
    rotation_notify: Arc<Notify>,
    retries: u32,
    background_task: tokio::task::JoinHandle<()>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.background_task.abort();
    }
}

#[derive(Deserialize, Debug)]
pub struct ClientConfig {
    pub endpoints: Vec<String>,
    #[serde(default = "bool_true")]
    pub shuffle_endpoints: bool,
    pub health_check: Option<HealthCheckConfig>,
}

pub fn bool_true() -> bool {
    true
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct HealthCheckConfig {
    #[serde(default = "interval_sec")]
    pub interval_sec: u64,
    #[serde(default = "healthy_response_time_ms")]
    pub healthy_response_time_ms: u64,
    #[serde(default = "system_health")]
    pub health_method: String,
}

pub fn interval_sec() -> u64 {
    10
}

pub fn healthy_response_time_ms() -> u64 {
    500
}

pub fn system_health() -> String {
    "system_health".to_string()
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
        let health_check = config.health_check.clone();
        if config.shuffle_endpoints {
            let mut endpoints = config.endpoints.clone();
            endpoints.shuffle(&mut thread_rng());
            Ok(Self::new(endpoints, None, None, None, health_check)?)
        } else {
            Ok(Self::new(config.endpoints.clone(), None, None, None, health_check)?)
        }
    }
}

impl Client {
    pub fn new(
        endpoints: impl IntoIterator<Item = impl AsRef<str>>,
        request_timeout: Option<Duration>,
        connection_timeout: Option<Duration>,
        retries: Option<u32>,
        health_config: Option<HealthCheckConfig>,
    ) -> Result<Self, anyhow::Error> {
        let health_config = health_config.unwrap_or_default();
        let endpoints: Vec<_> = endpoints.into_iter().map(|e| e.as_ref().to_string()).collect();

        if endpoints.is_empty() {
            return Err(anyhow!("No endpoints provided"));
        }

        if let Some(0) = retries {
            return Err(anyhow!("Retries need to be at least 1"));
        }

        tracing::debug!("New client with endpoints: {:?}", endpoints);

        let endpoints = endpoints
            .into_iter()
            .map(|e| {
                Arc::new(Endpoint::new(
                    e,
                    request_timeout,
                    connection_timeout,
                    health_config.clone(),
                ))
            })
            .collect::<Vec<_>>();

        let (message_tx, mut message_rx) = tokio::sync::mpsc::channel::<Message>(100);

        let message_tx_bg = message_tx.clone();

        let rotation_notify = Arc::new(Notify::new());
        let rotation_notify_bg = rotation_notify.clone();

        let background_task = tokio::spawn(async move {
            let request_backoff_counter = Arc::new(AtomicU32::new(0));

            // Select next endpoint with the highest health score, excluding the current one if provided
            let healthiest_endpoint = |exclude: Option<Arc<Endpoint>>| async {
                if endpoints.len() == 1 {
                    let selected_endpoint = endpoints[0].clone();
                    // Ensure it's connected
                    selected_endpoint.connected().await;
                    return selected_endpoint;
                }

                let mut endpoints = endpoints.clone();
                // Remove the current endpoint from the list
                if let Some(exclude) = exclude {
                    endpoints.retain(|e| e.url != exclude.url);
                }
                // Sort by health score
                endpoints.sort_by(|a, b| b.health.score().cmp(&a.health.score()));
                // Pick the first one
                let selected_endpoint = endpoints[0].clone();
                // Ensure it's connected
                selected_endpoint.connected().await;
                selected_endpoint
            };

            let mut selected_endpoint = healthiest_endpoint(None).await;

            let handle_message = |message: Message, endpoint: Arc<Endpoint>| {
                let tx = message_tx_bg.clone();
                let request_backoff_counter = request_backoff_counter.clone();

                // total timeout for a request
                let task_timeout = request_timeout.unwrap_or(Duration::from_secs(30));

                tokio::spawn(async move {
                    match message {
                        Message::Request {
                            method,
                            params,
                            response,
                            mut retries,
                        } => {
                            retries = retries.saturating_sub(1);

                            // make sure it's still connected
                            if response.is_closed() {
                                return;
                            }

                            if let Ok(result) =
                                tokio::time::timeout(task_timeout, endpoint.request(&method, params.clone())).await
                            {
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
                                                tx.send(Message::RotateEndpoint)
                                                    .await
                                                    .expect("Failed to send rotate message");

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

                                                tx.send(Message::Request {
                                                    method,
                                                    params,
                                                    response,
                                                    retries,
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
                            } else {
                                tracing::error!("request timed out method: {} params: {:?}", method, params);
                                // make sure it's still connected
                                if response.is_closed() {
                                    return;
                                }
                                let _ = response.send(Err(Error::RequestTimeout));
                            }
                        }
                        Message::Subscribe {
                            subscribe,
                            params,
                            unsubscribe,
                            response,
                            mut retries,
                        } => {
                            retries = retries.saturating_sub(1);

                            if let Ok(result) = tokio::time::timeout(
                                task_timeout,
                                endpoint.subscribe(&subscribe, params.clone(), &unsubscribe),
                            )
                            .await
                            {
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
                                                tx.send(Message::RotateEndpoint)
                                                    .await
                                                    .expect("Failed to send rotate message");

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

                                                tx.send(Message::Subscribe {
                                                    subscribe,
                                                    params,
                                                    unsubscribe,
                                                    response,
                                                    retries,
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
                            } else {
                                tracing::error!("subscribe timed out subscribe: {} params: {:?}", subscribe, params);
                                // make sure it's still connected
                                if response.is_closed() {
                                    return;
                                }
                                let _ = response.send(Err(Error::RequestTimeout));
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
                    _ = selected_endpoint.on_client_unhealthy.notified() => {
                        // Current selected endpoint is unhealthy, try to rotate to another one.
                        // In case of all endpoints are unhealthy, we don't want to keep rotating but stick with the healthiest one.
                        let new_selected_endpoint = healthiest_endpoint(None).await;
                        if new_selected_endpoint.url != selected_endpoint.url {
                            tracing::info!("Endpoint {current_url} is unhealthy, switch to endpoint: {new_url}", current_url = selected_endpoint.url, new_url=new_selected_endpoint.url);
                            selected_endpoint = new_selected_endpoint;
                            rotation_notify_bg.notify_waiters();
                        }
                    }
                    message = message_rx.recv() => {
                        tracing::trace!("Received message {message:?}");
                        match message {
                            Some(Message::RotateEndpoint) => {
                                tracing::info!("Rotating endpoint ...");
                                selected_endpoint = healthiest_endpoint(Some(selected_endpoint.clone())).await;
                                rotation_notify_bg.notify_waiters();
                            }
                            Some(message) => handle_message(message, selected_endpoint.clone()),
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
            sender: message_tx,
            rotation_notify,
            retries: retries.unwrap_or(3),
            background_task,
        })
    }

    pub fn with_endpoints(endpoints: impl IntoIterator<Item = impl AsRef<str>>) -> Result<Self, anyhow::Error> {
        Self::new(endpoints, None, None, None, None)
    }

    pub async fn request(&self, method: &str, params: Vec<JsonValue>) -> CallResult {
        async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.sender
                .send(Message::Request {
                    method: method.into(),
                    params,
                    response: tx,
                    retries: self.retries,
                })
                .await
                .map_err(errors::internal_error)?;

            rx.await.map_err(errors::internal_error)?.map_err(errors::map_error)
        }
        .with_context(TRACER.context(method.to_string()))
        .await
    }

    pub async fn subscribe(
        &self,
        subscribe: &str,
        params: Vec<JsonValue>,
        unsubscribe: &str,
    ) -> Result<Subscription<JsonValue>, Error> {
        async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.sender
                .send(Message::Subscribe {
                    subscribe: subscribe.into(),
                    params,
                    unsubscribe: unsubscribe.into(),
                    response: tx,
                    retries: self.retries,
                })
                .await
                .map_err(errors::internal_error)?;

            rx.await.map_err(errors::internal_error)?
        }
        .with_context(TRACER.context(subscribe.to_string()))
        .await
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

pub fn get_backoff_time(counter: &Arc<AtomicU32>) -> Duration {
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
