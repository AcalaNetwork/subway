use async_trait::async_trait;
use blake2::{Blake2b512, Digest};
use jsonrpsee::{
    core::{JsonValue, SubscriptionCallbackError},
    SubscriptionMessage,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
    io::Write,
    sync::Arc,
    time::Duration,
};
use tokio::sync::{broadcast, RwLock};

use super::{Middleware, NextFn};
use crate::{client::Client, config::MergeStrategy, middleware::subscription::SubscriptionRequest};

#[derive(Serialize, Deserialize, Debug)]
struct StorageChanges {
    block: String,
    changes: Vec<(String, Option<String>)>,
}

fn merge_storage_changes(
    current_value: JsonValue,
    new_value: JsonValue,
) -> Result<JsonValue, serde_json::Error> {
    let mut current = serde_json::from_value::<StorageChanges>(current_value)?;
    let StorageChanges { block, changes } = serde_json::from_value::<StorageChanges>(new_value)?;

    let changed_keys = changes
        .clone()
        .into_iter()
        .map(|(key, _)| key)
        .collect::<BTreeSet<_>>();

    // replace block hash
    current.block = block;
    // remove changed keys
    current
        .changes
        .retain(|(key, _)| !changed_keys.contains(key));
    // append new changes
    current.changes.extend(changes);

    serde_json::to_value(current)
}

fn handle_value_change(
    merge_strategy: MergeStrategy,
    current_value: Option<JsonValue>,
    new_value: JsonValue,
) -> JsonValue {
    if let Some(current_value) = current_value {
        match merge_strategy {
            MergeStrategy::Replace => new_value,
            MergeStrategy::MergeStorageChanges => {
                merge_storage_changes(current_value, new_value.clone()).unwrap_or(new_value)
            }
        }
    } else {
        new_value
    }
}

type UpstreamSubscription = broadcast::Sender<SubscriptionMessage>;

pub struct MergeSubscriptionMiddleware {
    client: Arc<Client>,
    merge_strategy: MergeStrategy,
    upstream_subs: Arc<RwLock<HashMap<[u8; 64], UpstreamSubscription>>>,
    current_values: Arc<RwLock<HashMap<[u8; 64], JsonValue>>>,
}

impl MergeSubscriptionMiddleware {
    pub fn new(client: Arc<Client>, merge_strategy: MergeStrategy) -> Self {
        Self {
            client,
            merge_strategy,
            upstream_subs: Arc::new(RwLock::new(HashMap::new())),
            current_values: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn get_upstream_subscription(
        &self,
        key: [u8; 64],
        subscribe: String,
        params: Vec<JsonValue>,
        unsubscribe: String,
    ) -> Result<broadcast::Receiver<SubscriptionMessage>, SubscriptionCallbackError> {
        if let Some(tx) = self.upstream_subs.read().await.get(&key).cloned() {
            log::trace!("Found existing upstream subscription for {}", &subscribe);
            return Ok(tx.subscribe());
        }

        log::trace!("Create new upstream subscription for {}", &subscribe);

        let mut subscription = self
            .client
            .subscribe(&subscribe, params.clone(), &unsubscribe)
            .await
            .map_err(|e| SubscriptionCallbackError::Some(e.to_string()))?;

        let (tx, rx) = broadcast::channel(1);
        self.upstream_subs.write().await.insert(key, tx.clone());

        let merge_strategy = self.merge_strategy;
        let client = self.client.clone();
        let upstream_subs = self.upstream_subs.clone();
        let current_values = self.current_values.clone();

        tokio::spawn(async move {
            // this ticker acts like a waker to help cleanup subscriptions
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // The first tick completes immediately
            interval.tick().await;

            loop {
                tokio::select! {
                    resp = subscription.next() => {
                        if tx.receiver_count() == 0 { break; }

                        interval.reset();

                        if let Some(Ok(value)) = resp {
                            // update current value
                            let current_value = current_values.read().await.get(&key).cloned();
                            current_values.write().await.insert(key, handle_value_change(merge_strategy, current_value, value.clone()));

                            // broadcast value change
                            if let Ok(message) = SubscriptionMessage::from_json(&value) {
                                if let Err(err) = tx.send(message.clone()) {
                                    log::error!("Failed to send message: {}", err);
                                    break;
                                }
                            }
                        } else {
                            match client.subscribe(&subscribe, params.clone(), &unsubscribe).await {
                                Ok(new_subscription) => {
                                    subscription = new_subscription;
                                }
                                Err(err) => {
                                    log::error!("failed to resubscribe {:?}", err);
                                    break;
                                }
                            }
                        }
                    }
                    _ = interval.tick() => {
                        // break if no other receiver than the one on hash map
                        if tx.receiver_count() == 0 { break; }
                    }
                }
            }

            upstream_subs.write().await.remove(&key);
            current_values.write().await.remove(&key);
            if let Err(err) = subscription.unsubscribe().await {
                log::error!("Failed to unsubscription {:?}", err);
            }
        });

        Ok(rx)
    }
}

#[async_trait]
impl Middleware<SubscriptionRequest, Result<(), SubscriptionCallbackError>>
    for MergeSubscriptionMiddleware
{
    async fn call(
        &self,
        request: SubscriptionRequest,
        _next: NextFn<SubscriptionRequest, Result<(), SubscriptionCallbackError>>,
    ) -> Result<(), SubscriptionCallbackError> {
        let mut hasher = Blake2b512::new();
        hasher
            .write_all(request.subscribe.as_bytes())
            .expect("should not fail");
        for p in &request.params {
            hasher
                .write_all(p.to_string().as_bytes())
                .expect("should not fail");
        }
        let key: [u8; 64] = hasher.finalize().into();

        let sink = request.sink.accept().await?;

        // send current value
        if let Some(current) = self.current_values.read().await.get(&key) {
            if let Ok(message) = SubscriptionMessage::from_json(&current) {
                if (sink.send(message).await).is_err() {
                    return Ok(());
                }
            }
        }

        let mut stream = self
            .get_upstream_subscription(
                key,
                request.subscribe.to_owned(),
                request.params.to_owned(),
                request.unsubscribe,
            )
            .await?;

        // broadcast new values
        tokio::spawn(async move {
            // this ticker acts like a waker to help cleanup sinks
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // The first tick completes immediately
            interval.tick().await;

            loop {
                tokio::select! {
                    resp = stream.recv() => {
                        if sink.is_closed() { break; }
                        match resp {
                            Err(_) => { break; }
                            Ok(new_value) => {
                                interval.reset();
                                if (sink.send(new_value).await).is_err() { break; }
                            }
                        }
                    }
                    _ = interval.tick() => {
                        if sink.is_closed() { break; }
                    }
                }
            }
        });

        Ok(())
    }
}
