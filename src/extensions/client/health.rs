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
    pub unhealthy: tokio::sync::Notify,
}

const MAX_SCORE: u32 = 100;
const THRESHOLD: u32 = MAX_SCORE / 2;

impl Health {
    pub fn new(url: String, config: HealthCheckConfig) -> Self {
        Self {
            url,
            config,
            score: AtomicU32::new(100),
            unhealthy: tokio::sync::Notify::new(),
        }
    }

    pub fn score(&self) -> u32 {
        self.score.load(Ordering::Relaxed)
    }

    pub fn update(&self, event: Event) {
        let current_score = self.score.load(Ordering::Relaxed);
        let new_score = u32::min(
            match event {
                Event::ResponseOk => current_score.saturating_add(5),
                Event::SlowResponse => current_score.saturating_sub(5),
                Event::RequestTimeout | Event::ConnectionFailed | Event::StaleChain => 0,
                Event::ConnectionSuccessful => 100,
            },
            MAX_SCORE,
        );
        self.score.store(new_score, Ordering::Relaxed);
        // Notify waiters if the score has dropped below the threshold
        if current_score >= THRESHOLD && new_score < THRESHOLD {
            self.unhealthy.notify_waiters();
        }
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

impl Health {
    pub fn monitor(
        health: Arc<Health>,
        client_rx_: tokio::sync::watch::Receiver<Option<Arc<Client>>>,
        on_client_ready: Arc<tokio::sync::Notify>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // Wait for the client to be ready before starting the health check
            on_client_ready.notified().await;

            let method_name = health.config.health_method.as_str();
            let interval = Duration::from_secs(health.config.interval_sec);
            let healthy_response_time = Duration::from_millis(health.config.healthy_response_time_ms);

            let client = match client_rx_.borrow().clone() {
                Some(client) => client,
                None => return,
            };

            loop {
                // Wait for the next interval
                tokio::time::sleep(interval).await;

                let request_start = std::time::Instant::now();
                match client
                    .request::<serde_json::Value, Vec<serde_json::Value>>(method_name, vec![])
                    .await
                {
                    Ok(response) => {
                        let duration = request_start.elapsed();

                        // Check known health responses
                        match method_name {
                            "system_health" => {
                                // Substrate node
                                if let Some(true) = response.get("isSyncing").and_then(|x| x.as_bool()) {
                                    health.update(Event::StaleChain);
                                    continue;
                                }
                            }
                            "net_health" => {
                                // Eth-RPC-Adapter (bodhijs)
                                if let Some(false) = response.get("isHealthy").and_then(|x| x.as_bool()) {
                                    health.update(Event::StaleChain);
                                    continue;
                                }
                            }
                            "eth_syncing" => {
                                // Ethereum node
                                if response.as_bool().unwrap_or(true) {
                                    health.update(Event::StaleChain);
                                    continue;
                                }
                            }
                            _ => {}
                        };

                        // Check response time
                        if duration > healthy_response_time {
                            health.update(Event::SlowResponse);
                            continue;
                        }

                        health.update(Event::ResponseOk);
                    }
                    Err(err) => {
                        health.on_error(&err);
                    }
                }
            }
        })
    }
}
