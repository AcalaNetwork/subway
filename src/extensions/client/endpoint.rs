use super::health::{Event, Health};
use crate::{
    extensions::client::{get_backoff_time, HealthCheckConfig},
    utils::errors,
};
use jsonrpsee::{
    async_client::Client,
    core::client::{ClientT, Subscription, SubscriptionClientT},
    ws_client::WsClientBuilder,
};
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};

pub struct Endpoint {
    url: String,
    health: Arc<Health>,
    client_rx: tokio::sync::watch::Receiver<Option<Arc<Client>>>,
    on_client_ready: Arc<tokio::sync::Notify>,
    background_tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.background_tasks.drain(..).for_each(|handle| handle.abort());
    }
}

impl Endpoint {
    pub fn new(
        url: String,
        request_timeout: Option<Duration>,
        connection_timeout: Option<Duration>,
        health_config: HealthCheckConfig,
    ) -> Self {
        let (client_tx, client_rx) = tokio::sync::watch::channel(None);
        let on_client_ready = Arc::new(tokio::sync::Notify::new());
        let health = Arc::new(Health::new(url.clone(), health_config));

        let url_ = url.clone();
        let health_ = health.clone();
        let on_client_ready_ = on_client_ready.clone();

        // This task will try to connect to the endpoint and keep the connection alive
        let connection_task = tokio::spawn(async move {
            let connect_backoff_counter = Arc::new(AtomicU32::new(0));

            loop {
                tracing::info!("Connecting endpoint: {url_}");

                let client = WsClientBuilder::default()
                    .request_timeout(request_timeout.unwrap_or(Duration::from_secs(30)))
                    .connection_timeout(connection_timeout.unwrap_or(Duration::from_secs(30)))
                    .max_buffer_capacity_per_subscription(2048)
                    .max_concurrent_requests(2048)
                    .max_response_size(20 * 1024 * 1024)
                    .build(&url_);

                match client.await {
                    Ok(client) => {
                        let client = Arc::new(client);
                        health_.update(Event::ConnectionSuccessful);
                        _ = client_tx.send(Some(client.clone()));
                        on_client_ready_.notify_waiters();
                        tracing::info!("Endpoint connected: {url_}");
                        connect_backoff_counter.store(0, Ordering::Relaxed);
                        client.on_disconnect().await;
                    }
                    Err(err) => {
                        health_.on_error(&err);
                        _ = client_tx.send(None);
                        tracing::warn!("Unable to connect to endpoint: {url_} error: {err}");
                    }
                }
                // Wait a bit before trying to reconnect
                tokio::time::sleep(get_backoff_time(&connect_backoff_counter)).await;
            }
        });

        // This task will check the health of the endpoint and update the health score
        let health_checker = Health::monitor(health.clone(), client_rx.clone(), on_client_ready.clone());

        Self {
            url,
            health,
            client_rx,
            on_client_ready,
            background_tasks: vec![connection_task, health_checker],
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn health(&self) -> &Health {
        self.health.as_ref()
    }

    pub async fn connected(&self) {
        if self.client_rx.borrow().is_some() {
            return;
        }
        self.on_client_ready.notified().await;
    }

    pub async fn request(
        &self,
        method: &str,
        params: Vec<serde_json::Value>,
        timeout: Duration,
    ) -> Result<serde_json::Value, jsonrpsee::core::client::Error> {
        let client = self
            .client_rx
            .borrow()
            .clone()
            .ok_or(errors::failed("client not connected"))?;

        match tokio::time::timeout(timeout, client.request(method, params.clone())).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(err)) => {
                self.health.on_error(&err);
                Err(err)
            }
            Err(_) => {
                tracing::error!("request timed out method: {method} params: {params:?}");
                self.health.on_error(&jsonrpsee::core::client::Error::RequestTimeout);
                Err(jsonrpsee::core::client::Error::RequestTimeout)
            }
        }
    }

    pub async fn subscribe(
        &self,
        subscribe_method: &str,
        params: Vec<serde_json::Value>,
        unsubscribe_method: &str,
        timeout: Duration,
    ) -> Result<Subscription<serde_json::Value>, jsonrpsee::core::client::Error> {
        let client = self
            .client_rx
            .borrow()
            .clone()
            .ok_or(errors::failed("client not connected"))?;

        match tokio::time::timeout(
            timeout,
            client.subscribe(subscribe_method, params.clone(), unsubscribe_method),
        )
        .await
        {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(err)) => {
                self.health.on_error(&err);
                Err(err)
            }
            Err(_) => {
                tracing::error!("subscribe timed out subscribe: {subscribe_method} params: {params:?}");
                self.health.on_error(&jsonrpsee::core::client::Error::RequestTimeout);
                Err(jsonrpsee::core::client::Error::RequestTimeout)
            }
        }
    }
}
