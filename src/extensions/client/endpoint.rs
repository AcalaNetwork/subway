use crate::{extensions::client::get_backoff_time, utils::errors};
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

#[derive(Debug)]
pub enum Event {
    ResponseOk,
    SlowResponse,
    RequestTimeout,
    ConnectionSuccessful,
    ConnectionFailed,
    StaleChain,
}

#[derive(Debug, Default)]
pub struct Health {
    pub score: AtomicU32,
}

impl Health {
    pub fn score(&self) -> u32 {
        self.score.load(Ordering::Relaxed)
    }

    pub fn update(&self, event: Event) {
        const MAX_SCORE: u32 = 100;
        let current_score = self.score.load(Ordering::Relaxed);
        let new_score = u32::min(
            match event {
                Event::ResponseOk => current_score.saturating_add(1),
                Event::SlowResponse => current_score.saturating_sub(5),
                Event::RequestTimeout => current_score.saturating_sub(10),
                Event::ConnectionFailed | Event::StaleChain => 0,
                Event::ConnectionSuccessful => 100,
            },
            MAX_SCORE,
        );
        self.score.store(new_score, Ordering::Relaxed);
    }

    pub fn on_error(&self, err: &jsonrpsee::core::Error) {
        match err {
            jsonrpsee::core::Error::RequestTimeout => {
                self.update(Event::RequestTimeout);
            }
            jsonrpsee::core::Error::Transport(_)
            | jsonrpsee::core::Error::RestartNeeded(_)
            | jsonrpsee::core::Error::MaxSlotsExceeded => {
                self.update(Event::ConnectionFailed);
            }
            _ => {}
        };
    }
}

pub struct Endpoint {
    pub url: String,
    pub health: Arc<Health>,
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
    pub fn new(url: String, request_timeout: Option<Duration>, connection_timeout: Option<Duration>) -> Self {
        let (client_tx, client_rx) = tokio::sync::watch::channel(None);
        let on_client_ready = Arc::new(tokio::sync::Notify::new());
        let health = Arc::new(Health::default());

        let url_ = url.clone();
        let health_ = health.clone();
        let on_client_ready_ = on_client_ready.clone();

        // This task will try to connect to the endpoint and keep the connection alive
        let connection_task = tokio::spawn(async move {
            let connect_backoff_counter = Arc::new(AtomicU32::new(0));

            loop {
                tracing::info!("Connecting to endpoint: {url_}");

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
                        tracing::warn!("Unable to connect to endpoint: '{url_}' error: {err}");
                        tokio::time::sleep(get_backoff_time(&connect_backoff_counter)).await;
                    }
                }
                // Wait a second before trying to reconnect
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });

        // This task will check the health of the endpoint and update the health score
        let url_ = url.clone();
        let health_ = health.clone();
        let on_client_ready_ = on_client_ready.clone();
        let client_rx_ = client_rx.clone();
        let health_checker = tokio::spawn(async move {
            // Wait for the client to be ready before starting the health check
            on_client_ready_.notified().await;

            let method_name = "system_health";
            let interval = Duration::from_secs(10);
            let client = match client_rx_.borrow().clone() {
                Some(client) => client,
                None => return,
            };

            // Check if the endpoint has the 'system_health' method
            match client
                .request::<serde_json::Value, Vec<serde_json::Value>>("rpc_methods", vec![])
                .await
            {
                Ok(response) => {
                    let has_health_method = response
                        .get("methods")
                        .unwrap_or(&serde_json::json!([]))
                        .as_array()
                        .map(|methods| methods.iter().any(|x| x.as_str() == Some(method_name)))
                        .unwrap_or_default();
                    if !has_health_method {
                        tracing::warn!("Endpoint '{url_}' does not have the '{method_name}' method");
                        return;
                    }
                }
                Err(_) => return,
            };

            loop {
                tokio::time::sleep(interval).await;

                let client = match client_rx_.borrow().clone() {
                    Some(client) => client,
                    None => continue,
                };

                let request_start = std::time::Instant::now();
                match client
                    .request::<serde_json::Value, Vec<serde_json::Value>>(method_name, vec![])
                    .await
                {
                    Ok(response) => {
                        let is_syncing = response.get("isSyncing").unwrap_or(&serde_json::json!(false));
                        if is_syncing.as_bool() == Some(true) {
                            health_.update(Event::StaleChain);
                            continue;
                        }
                        // TODO: make this configurable
                        if request_start.elapsed().as_millis() > 250 {
                            health_.update(Event::SlowResponse);
                            continue;
                        }
                        health_.update(Event::ResponseOk);
                    }
                    Err(err) => {
                        health_.on_error(&err);
                    }
                }
            }
        });

        Self {
            url,
            health,
            client_rx,
            on_client_ready,
            background_tasks: vec![connection_task, health_checker],
        }
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
    ) -> Result<serde_json::Value, jsonrpsee::core::Error> {
        let client = self
            .client_rx
            .borrow()
            .clone()
            .ok_or(errors::failed("client not connected"))?;

        match client.request(method, params.clone()).await {
            Ok(response) => {
                self.health.update(Event::ResponseOk);
                Ok(response)
            }
            Err(err) => {
                self.health.on_error(&err);
                Err(err)
            }
        }
    }

    pub async fn subscribe(
        &self,
        subscribe_method: &str,
        params: Vec<serde_json::Value>,
        unsubscribe_method: &str,
    ) -> Result<Subscription<serde_json::Value>, jsonrpsee::core::Error> {
        let client = self
            .client_rx
            .borrow()
            .clone()
            .ok_or(errors::failed("client not connected"))?;

        match client
            .subscribe(subscribe_method, params.clone(), unsubscribe_method)
            .await
        {
            Ok(response) => {
                self.health.update(Event::ResponseOk);
                Ok(response)
            }
            Err(err) => {
                self.health.on_error(&err);
                Err(err)
            }
        }
    }
}
