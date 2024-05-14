use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Debug)]
pub enum Event {
    ResponseOk,
    ConnectionSuccessful,
    SlowResponse,
    RequestTimeout,
    ServerError,
    Unhealthy,
    ConnectionClosed,
}

impl Event {
    pub fn update_score(&self, current: u32) -> u32 {
        u32::min(
            match self {
                Event::ResponseOk => current.saturating_add(2),
                Event::SlowResponse => current.saturating_sub(5),
                Event::RequestTimeout | Event::ServerError | Event::Unhealthy | Event::ConnectionClosed => 0,
                Event::ConnectionSuccessful => MAX_SCORE / 5 * 4, // 80% of max score
            },
            MAX_SCORE,
        )
    }
}

#[derive(Debug, Default)]
pub struct Health {
    url: String,
    score: AtomicU32,
    unhealthy: tokio::sync::Notify,
}

const MAX_SCORE: u32 = 100;
const THRESHOLD: u32 = MAX_SCORE / 2;

impl Health {
    pub fn new(url: String) -> Self {
        Self {
            url,
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

    pub fn on_error(&self, err: &jsonrpsee::core::Error) {
        match err {
            jsonrpsee::core::Error::Call(_) => {
                // NOT SERVER ERROR
            }
            jsonrpsee::core::Error::RequestTimeout => {
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
