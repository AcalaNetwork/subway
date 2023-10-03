use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use jsonrpsee::core::JsonValue;
use serde::Deserialize;
use tokio::sync::watch;

use crate::{
    extension::Extension,
    extensions::{
        api::{BaseApi, ValueHandle},
        client::Client,
    },
    middleware::ExtensionRegistry,
};

pub struct SubstrateApi {
    client: Arc<Client>,
    inner: BaseApi,
    stale_timeout: Duration,
}

#[derive(Deserialize, Debug)]
pub struct SubstrateApiConfig {
    stale_timeout_seconds: u64,
}

#[async_trait]
impl Extension for SubstrateApi {
    type Config = SubstrateApiConfig;

    async fn from_config(
        config: &Self::Config,
        registry: &ExtensionRegistry,
    ) -> Result<Self, anyhow::Error> {
        let client = registry.get::<Client>().await.expect("Client not found");

        Ok(Self::new(
            client,
            Duration::from_secs(config.stale_timeout_seconds),
        ))
    }
}

impl SubstrateApi {
    pub fn new(client: Arc<Client>, stale_timeout: Duration) -> Self {
        let (head_tx, head_rx) = watch::channel::<Option<(JsonValue, u64)>>(None);
        let (finalized_head_tx, finalized_head_rx) =
            watch::channel::<Option<(JsonValue, u64)>>(None);

        let this = Self {
            client,
            inner: BaseApi::new(head_rx, finalized_head_rx),
            stale_timeout,
        };

        this.start_background_task(head_tx, finalized_head_tx);

        this
    }

    pub fn get_head(&self) -> ValueHandle<(JsonValue, u64)> {
        self.inner.get_head()
    }

    pub fn get_finalized_head(&self) -> ValueHandle<(JsonValue, u64)> {
        self.inner.get_finalized_head()
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

            let client = client.clone();

            loop {
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

                                    let number = super::get_number(&val)?;

                                    let hash = client
                                        .request("chain_getBlockHash", vec![number.into()])
                                        .await?;

                                    tracing::debug!("New head: {number} {hash}");
                                    head_tx.send_replace(Some((hash, number)));
                                } else {
                                    break;
                                }
                            }
                            _ = interval.tick() => {
                                tracing::warn!("No new blocks for {stale_timeout:?} seconds, rotating endpoint");
                                client.rotate_endpoint().await;
                                break;
                            }
                            _ = client.on_rotation() => {
                                // endpoint is rotated, break the loop and restart subscription
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
                let run = async {
                    let mut sub = client
                        .subscribe(
                            "chain_subscribeFinalizedHeads",
                            [].into(),
                            "chain_unsubscribeFinalizedHeads",
                        )
                        .await?;

                    loop {
                        tokio::select! {
                            val = sub.next() => {
                                if let Some(Ok(val)) = val {
                                    let number = super::get_number(&val)?;

                                    let hash = client
                                        .request("chain_getBlockHash", vec![number.into()])
                                        .await?;

                                    if let Err(e) = super::validate_new_head(&finalized_head_tx, number, &hash)
                                    {
                                        tracing::error!("Error in background task: {e}");
                                        client.rotate_endpoint().await;
                                        break;
                                    }

                                    tracing::debug!("New finalized head: {number} {hash}");
                                    finalized_head_tx.send_replace(Some((hash, number)));
                                } else {
                                    break;
                                }
                            }
                            _ = client.on_rotation() => {
                                // endpoint is rotated, break the loop and restart subscription
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
