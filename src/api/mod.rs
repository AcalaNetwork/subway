use std::collections::LinkedList;
use std::{sync::Arc, time::Duration};

use jsonrpsee::core::JsonValue;
use tokio::sync::{watch, RwLock};

use crate::client::Client;
use crate::config::EthFinalization;

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
    pub fn new(
        client: Arc<Client>,
        stale_timeout: Duration,
        eth_finalization: Option<EthFinalization>,
    ) -> Self {
        let (head_tx, head_rx) = watch::channel::<Option<(JsonValue, u64)>>(None);
        let (finalized_head_tx, finalized_head_rx) =
            watch::channel::<Option<(JsonValue, u64)>>(None);

        let this = Self {
            client,
            head: head_rx,
            finalized_head: finalized_head_rx,
            stale_timeout,
        };

        if let Some(eth_finalization) = eth_finalization {
            this.start_eth_background_task(head_tx, finalized_head_tx, eth_finalization);
        } else {
            this.start_background_task(head_tx, finalized_head_tx);
        }

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
            let mut interval = tokio::time::interval(stale_timeout);

            loop {
                let client = client.clone();

                let run = async {
                    interval.reset();

                    let mut sub = client
                        .subscribe(
                            "chain_subscribeNewHeads",
                            [].into(),
                            "chain_unsubscribeNewHeads",
                        )
                        .await?;

                    loop {
                        tokio::select! {
                            val = sub.next() => {
                                if let Some(Ok(val)) = val {
                                    interval.reset();

                                    let number = get_number(&val)?;

                                    let res = client
                                        .request("chain_getBlockHash", vec![number.into()])
                                        .await?;

                                    tracing::debug!("New head: {number} {res}");
                                    head_tx.send_replace(Some((res, number)));
                                } else {
                                    break;
                                }
                            }
                            _ = interval.tick() => {
                                tracing::warn!("No new blocks for {stale_timeout:?} seconds, rotating endpoint");
                                client.rotate_endpoint().await.expect("Failed to rotate endpoint");
                                break;
                            }
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
                    while let Some(Ok(val)) = sub.next().await {
                        let number = get_number(&val)?;

                        let res = client
                            .request("chain_getBlockHash", vec![number.into()])
                            .await?;

                        tracing::debug!("New finalized head: {number} {res}");
                        finalized_head_tx.send_replace(Some((res, number)));
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

    fn start_eth_background_task(
        &self,
        head_tx: watch::Sender<Option<(JsonValue, u64)>>,
        finalized_head_tx: watch::Sender<Option<(JsonValue, u64)>>,
        eth_finalization: EthFinalization,
    ) {
        let client = self.client.clone();
        let stale_timeout = self.stale_timeout;

        let mut unfinalized_blocks = LinkedList::<(JsonValue, u64)>::new();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(stale_timeout);

            let (check_finalized_tx, mut check_finalized_rx) = tokio::sync::mpsc::channel::<()>(1);

            let check_finalized_tx2 = check_finalized_tx.clone();
            tokio::spawn(async move {
                let mut query_finalized_interval = tokio::time::interval(Duration::from_secs(1));
                loop {
                    query_finalized_interval.tick().await;
                    let _ = check_finalized_tx2.try_send(());
                }
            });

            loop {
                let client = client.clone();

                let run = async {
                    interval.reset();

                    // query current head
                    let head = client
                        .request("eth_getBlockByNumber", vec!["latest".into(), true.into()])
                        .await?;
                    let number = get_number(&head)?;
                    let hash: JsonValue = head["hash"].clone();

                    tracing::debug!("New head: {number} {hash}");
                    head_tx.send_replace(Some((hash.clone(), number)));
                    match eth_finalization {
                        EthFinalization::Latest => {
                            tracing::debug!("New finalized head: {number} {hash}");
                            finalized_head_tx.send_replace(Some((hash, number)));
                        }
                        _ => {
                            unfinalized_blocks.push_front((hash, number));
                            let _ = check_finalized_tx.try_send(());
                        }
                    }

                    let mut sub = client
                        .subscribe(
                            "eth_subscribe",
                            ["newHeads".into()].into(),
                            "eth_unsubscribe",
                        )
                        .await?;

                    loop {
                        tokio::select! {
                            val = sub.next() => {
                                if let Some(Ok(val)) = val {
                                    interval.reset();

                                    let number = get_number(&val)?;

                                    let hash: JsonValue = val["hash"].clone();

                                    tracing::debug!("New head: {number} {hash}");
                                    head_tx.send_replace(Some((hash.clone(), number)));
                                    match eth_finalization {
                                        EthFinalization::Latest => {
                                            tracing::debug!("New finalized head: {number} {hash}");
                                            finalized_head_tx.send_replace(Some((hash, number)));
                                        }
                                        _ => {
                                            unfinalized_blocks.push_front((hash, number));
                                            let _ = check_finalized_tx.try_send(());
                                        }
                                    }
                                } else {
                                    break;
                                }
                            }
                            _ = check_finalized_rx.recv() => {
                                if let Some((hash, number)) = unfinalized_blocks.back() {
                                    let res = client.request("eth_isBlockFinalized", vec![format!("0x{:x}", number).into()]).await?;
                                    let finalized = res.as_bool().ok_or_else(|| anyhow::Error::msg("expected boolean"))?;
                                    if finalized {
                                        tracing::debug!("New finalized head: {number} {hash}");
                                        finalized_head_tx.send_replace(Some((hash.to_owned(), number.to_owned())));
                                        unfinalized_blocks.pop_back();
                                        let _ = check_finalized_tx.try_send(());
                                    }
                                }
                            }
                            _ = interval.tick() => {
                                tracing::warn!("No new blocks for {stale_timeout:?} seconds, rotating endpoint");
                                client.rotate_endpoint().await.expect("Failed to rotate endpoint");
                                break;
                            }
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
