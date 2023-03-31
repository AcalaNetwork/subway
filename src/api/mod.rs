use jsonrpsee::core::JsonValue;
use std::sync::Arc;
use tokio::sync::{watch, RwLock};

use crate::client::Client;

#[cfg(test)]
mod tests;

pub mod eth;
pub mod substrate;

pub use eth::EthApi;
pub use substrate::SubstrateApi;

pub trait Api: Send + Sync {
    fn get_head(&self) -> ValueHandle<(JsonValue, u64)>;
    fn get_finalized_head(&self) -> ValueHandle<(JsonValue, u64)>;
    fn current_head(&self) -> Option<(JsonValue, u64)>;
    fn current_finalized_head(&self) -> Option<(JsonValue, u64)>;
}

pub struct ValueHandle<T> {
    inner: RwLock<watch::Receiver<Option<T>>>,
}

impl<T: Clone> ValueHandle<T> {
    pub async fn read(&self) -> T {
        let read_guard = self.inner.read().await;
        let val = (*read_guard).borrow().clone();
        drop(read_guard);

        if let Some(val) = val {
            return val;
        }

        let mut write_guard = self.inner.write().await;
        if let Err(e) = write_guard.changed().await {
            tracing::error!("Changed channel closed: {}", e);
        }

        let val = (*write_guard)
            .borrow()
            .clone()
            .expect("already awaited changed");
        val
    }
}

pub(crate) struct BaseApi {
    pub client: Arc<Client>,
    pub head_rx: watch::Receiver<Option<(JsonValue, u64)>>,
    pub finalized_head_rx: watch::Receiver<Option<(JsonValue, u64)>>,
}

impl BaseApi {
    pub fn get_head(&self) -> ValueHandle<(JsonValue, u64)> {
        ValueHandle {
            inner: RwLock::new(self.head_rx.clone()),
        }
    }

    // TODO use this later
    #[allow(dead_code)]
    pub fn get_finalized_head(&self) -> ValueHandle<(JsonValue, u64)> {
        ValueHandle {
            inner: RwLock::new(self.finalized_head_rx.clone()),
        }
    }
}

impl BaseApi {
    pub fn new(
        client: Arc<Client>,
        head_rx: watch::Receiver<Option<(JsonValue, u64)>>,
        finalized_head_rx: watch::Receiver<Option<(JsonValue, u64)>>,
    ) -> Self {
        Self {
            client,
            head_rx,
            finalized_head_rx,
        }
    }
}

pub(crate) fn get_number(val: &JsonValue) -> anyhow::Result<u64> {
    let number = val["number"]
        .as_str()
        .and_then(|s| s.strip_prefix("0x"))
        .ok_or_else(|| anyhow::Error::msg("Invalid number"))?;
    let number = u64::from_str_radix(number, 16)?;
    Ok(number)
}

pub(crate) fn get_hash(val: &JsonValue) -> anyhow::Result<JsonValue> {
    let hash = val["hash"].to_owned();
    if hash.is_string() {
        return Ok(hash);
    }
    Err(anyhow::Error::msg("Hash not found"))
}
