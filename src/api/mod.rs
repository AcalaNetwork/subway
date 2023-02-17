use std::{sync::Arc, time::Duration};

use jsonrpsee::core::JsonValue;
use tokio::{
    sync::{watch, RwLock},
    time::interval,
};

use crate::client::Client;

#[cfg(test)]
mod tests;

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

pub struct Api {
    client: Arc<Client>,
    head: watch::Receiver<Option<(JsonValue, u64)>>,
    finalized_head: watch::Receiver<Option<(JsonValue, u64)>>,
    stale_timeout: Duration,
}

impl Api {
    pub fn get_head(&self) -> ValueHandle<(JsonValue, u64)> {
        ValueHandle {
            inner: RwLock::new(self.head.clone()),
        }
    }

    // TODO use this later
    #[allow(dead_code)]
    pub fn get_finalized_head(&self) -> ValueHandle<(JsonValue, u64)> {
        ValueHandle {
            inner: RwLock::new(self.finalized_head.clone()),
        }
    }
}

impl Api {
    pub fn new(client: Arc<Client>, stale_timeout: Duration) -> Self {
        let (head_tx, head_rx) = watch::channel::<Option<(JsonValue, u64)>>(None);
        let (finalized_head_tx, finalized_head_rx) =
            watch::channel::<Option<(JsonValue, u64)>>(None);

        let this = Self {
            client,
            head: head_rx,
            finalized_head: finalized_head_rx,
            stale_timeout,
        };

        this.start_background_task(head_tx, finalized_head_tx);

        this
    }

    fn start_background_task(
        &self,
        head_tx: watch::Sender<Option<(JsonValue, u64)>>,
        finalized_head_tx: watch::Sender<Option<(JsonValue, u64)>>,
    ) {
        let client = self.client.clone();
        let stale_timeout = self.stale_timeout;

        tokio::spawn(async move {
            let mut heartbeat = interval(stale_timeout);

            loop {
                let client = client.clone();

                let run = async {
                    let sub = client
                        .subscribe(
                            "chain_subscribeNewHeads",
                            [].into(),
                            "chain_unsubscribeNewHeads",
                        )
                        .await?;

                    let mut sub = sub;

                    loop {
                        tokio::select! {
                            Some(Ok(val)) = sub.next() => {
                                heartbeat.reset();

                                let number = get_number(&val)?;

                                let res = client
                                    .request("chain_getBlockHash", vec![number.into()])
                                    .await?;

                                tracing::debug!("New head: {number} {res}");
                                head_tx.send_replace(Some((res, number)));
                            }
                            _ = heartbeat.tick() => {
                                tracing::warn!("Heartbeat timed out");
                                client.rotate_endpoint().await.expect("Failed to rotate endpoint");
                                break;
                            }
                            else => break,
                        }
                    }

                    Ok::<(), anyhow::Error>(())
                };

                if let Err(e) = run.await {
                    tracing::error!("Error in background task: {e}");
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });

        let client = self.client.clone();

        tokio::spawn(async move {
            loop {
                let client = client.clone();

                let run = async {
                    let sub = client
                        .subscribe(
                            "chain_subscribeFinalizedHeads",
                            [].into(),
                            "chain_unsubscribeFinalizedHeads",
                        )
                        .await?;

                    let mut sub = sub;
                    while let Some(val) = sub.next().await {
                        if let Ok(val) = val {
                            let number = get_number(&val)?;

                            let res = client
                                .request("chain_getBlockHash", vec![number.into()])
                                .await?;

                            tracing::debug!("New finalized head: {number} {res}");
                            finalized_head_tx.send_replace(Some((res, number)));
                        }
                    }

                    Ok::<(), anyhow::Error>(())
                };

                if let Err(e) = run.await {
                    tracing::error!("Error in background task: {e}");
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    }
}

fn get_number(val: &JsonValue) -> anyhow::Result<u64> {
    let number = val["number"]
        .as_str()
        .and_then(|s| s.strip_prefix("0x"))
        .ok_or_else(|| anyhow::Error::msg("Invalid number"))?;
    let number = u64::from_str_radix(number, 16)?;
    Ok(number)
}
