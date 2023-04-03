use jsonrpsee::core::JsonValue;
use std::sync::Arc;
use tokio::sync::watch;

use crate::client::Client;

#[cfg(test)]
mod tests;

mod eth;
mod substrate;
mod value_handle;

pub use eth::EthApi;
pub use substrate::SubstrateApi;
pub use value_handle::ValueHandle;

pub(crate) struct BaseApi {
    pub client: Arc<Client>,
    pub head_rx: watch::Receiver<Option<(JsonValue, u64)>>,
    pub finalized_head_rx: watch::Receiver<Option<(JsonValue, u64)>>,
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

    pub fn get_head(&self) -> ValueHandle<(JsonValue, u64)> {
        ValueHandle::new(self.head_rx.clone())
    }

    pub fn get_finalized_head(&self) -> ValueHandle<(JsonValue, u64)> {
        ValueHandle::new(self.finalized_head_rx.clone())
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
