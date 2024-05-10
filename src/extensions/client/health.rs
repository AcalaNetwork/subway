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
    ServerError,
    StaleChain,
}

impl Event {
    pub fn update_score(&self, current: u32) -> u32 {
        u32::min(
            match self {
                Event::ResponseOk => current.saturating_add(2),
                Event::SlowResponse => current.saturating_sub(5),
                Event::RequestTimeout | Event::ServerError | Event::StaleChain => 0,
                Event::ConnectionSuccessful => MAX_SCORE / 5 * 4, // 80% of max score
            },
            MAX_SCORE,
        )
    }
}

#[derive(Debug, Default)]
pub struct Health {
    url: String,
    config: HealthCheckConfig,
    score: AtomicU32,
    unhealthy: tokio::sync::Notify,
}

const MAX_SCORE: u32 = 100;
const THRESHOLD: u32 = MAX_SCORE / 2;

impl Health {
    pub fn new(url: String, config: HealthCheckConfig) -> Self {
        Self {
            url,
            config,
            score: AtomicU32::new(0),
            unhealthy: tokio::sync::Notify::new(),
        }
    }

    pub fn score(&self) -> u32 {
        self.score.load(Ordering::Relaxed)
    }

    pub fn update(&self, event: Event) {
        let current_score = self.score.load(Ordering::Relaxed);
        let new_score = event.update_score(current_score);
        if new_score == current_score {
            return;
        }
        self.score.store(new_score, Ordering::Relaxed);
        tracing::trace!(
            "Endpoint {:?} score updated from: {current_score} to: {new_score}",
            self.url
        );

        // Notify waiters if the score has dropped below the threshold
        if current_score >= THRESHOLD && new_score < THRESHOLD {
            tracing::warn!("Endpoint {:?} became unhealthy", self.url);
            self.unhealthy.notify_waiters();
        }
    }

    pub fn on_error(&self, err: &jsonrpsee::core::client::Error) {
        match err {
            jsonrpsee::core::client::Error::Call(_) => {
                // NOT SERVER ERROR
            }
            jsonrpsee::core::client::Error::RequestTimeout => {
                tracing::warn!("Endpoint {:?} request timeout", self.url);
                self.update(Event::RequestTimeout);
            }
            _ => {
                tracing::warn!("Endpoint {:?} responded with error: {err:?}", self.url);
                self.update(Event::ServerError);
            }
        };
    }

    pub async fn unhealthy(&self) {
        self.unhealthy.notified().await;
    }
}

impl Health {
    pub fn monitor(
        health: Arc<Health>,
        client_rx_: tokio::sync::watch::Receiver<Option<Arc<Client>>>,
        on_client_ready: Arc<tokio::sync::Notify>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // no health method
            if health.config.health_method.is_none() {
                return;
            }

            // Wait for the client to be ready before starting the health check
            on_client_ready.notified().await;

            let method_name = health.config.health_method.as_ref().expect("checked above");
            let health_response = health.config.response.clone();
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

                        // Check response
                        if let Some(ref health_response) = health_response {
                            if !health_response.validate(&response) {
                                health.update(Event::StaleChain);
                                continue;
                            }
                        }

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
