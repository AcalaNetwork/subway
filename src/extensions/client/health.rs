use crate::extensions::client::HealthCheckConfig;
use jsonrpsee::{async_client::Client, core::client::ClientT};
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
    pub url: String,
    pub config: HealthCheckConfig,
    pub score: AtomicU32,
}

impl Health {
    pub fn new(url: String, config: HealthCheckConfig) -> Self {
        Self {
            url,
            config,
            score: AtomicU32::new(100),
        }
    }

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

    pub async fn monitor(&self, client_rx_: tokio::sync::watch::Receiver<Option<Arc<Client>>>) {
        if self.config.health_method.is_empty() {
            return;
        }

        let method_name = self.config.health_method.as_str();
        let interval = Duration::from_secs(self.config.interval_sec);
        let response_threshold = Duration::from_micros(self.config.response_threshold_ms);

        let client = match client_rx_.borrow().clone() {
            Some(client) => client,
            None => return,
        };

        // Check if the endpoint has the health method
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
                    tracing::warn!(
                        "Endpoint {url} does not have the {method_name:?} method",
                        url = self.url
                    );
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
                    let duration = request_start.elapsed();

                    // Check if the node is syncing
                    if response
                        .get("isSyncing")
                        .unwrap_or(&serde_json::json!(false))
                        .as_bool()
                        .unwrap_or_default()
                    {
                        self.update(Event::StaleChain);
                        continue;
                    }

                    // Check response time
                    if duration > response_threshold {
                        self.update(Event::SlowResponse);
                        continue;
                    }

                    self.update(Event::ResponseOk);
                }
                Err(err) => {
                    self.on_error(&err);
                }
            }
        }
    }
}
