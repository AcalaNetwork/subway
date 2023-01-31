use std::{sync::Arc, time::Duration};

use jsonrpsee::core::JsonValue;
use tokio::sync::RwLock;

use crate::client::Client;

pub struct Api {
    client: Arc<Client>,
    head: Arc<RwLock<Option<(JsonValue, u64)>>>,
    finalized_head: Arc<RwLock<Option<(JsonValue, u64)>>>,
}

impl Api {
    pub fn new(client: Arc<Client>) -> Self {
        let this = Self {
            client,
            head: Arc::new(RwLock::new(None)),
            finalized_head: Arc::new(RwLock::new(None)),
        };

        this.start_background_task();

        this
    }

    fn start_background_task(&self) {
        let client = self.client.clone();
        let head = self.head.clone();

        tokio::spawn(async move {
            loop {
                let client = client.clone();
                let head = head.clone();

                let run = async move {
                    let sub = client
                        .subscribe(
                            "chain_subscribeNewHeads",
                            [].into(),
                            "chain_unsubscribeNewHeads",
                        )
                        .await?;

                    let mut sub = sub;
                    while let Some(val) = sub.next().await {
                        if let Ok(val) = val {
                            let number = get_number(&val)?;

                            let res = client
                                .request("chain_getBlockHash", vec![number.into()])
                                .await?;

                            tracing::debug!("New head: {number} {res}");
                            head.write().await.replace((res, number));
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
        let finalized_head = self.finalized_head.clone();

        tokio::spawn(async move {
            loop {
                let client = client.clone();
                let finalized_head = finalized_head.clone();

                let run = async move {
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
                            finalized_head.write().await.replace((res, number));
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
