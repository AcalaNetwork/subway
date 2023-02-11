use async_trait::async_trait;
use blake2::{Blake2b512, Digest};
use jsonrpsee::{
    core::{JsonValue, SubscriptionCallbackError},
    SubscriptionMessage,
};
use std::{collections::HashMap, io::Write, sync::Arc, time::Duration};
use tokio::sync::{watch, RwLock};

use super::{Middleware, NextFn};
use crate::{client::Client, middleware::subscription::SubscriptionRequest};

type UpstreamSubscription = watch::Receiver<Option<SubscriptionMessage>>;

pub struct MergeSubscriptionMiddleware {
    client: Arc<Client>,
    upstream_subs: Arc<RwLock<HashMap<[u8; 64], UpstreamSubscription>>>,
}

impl MergeSubscriptionMiddleware {
    pub fn new(client: Arc<Client>) -> Self {
        Self {
            client,
            upstream_subs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn get_upstream_subscription(
        &self,
        key: [u8; 64],
        subscribe: String,
        params: Vec<JsonValue>,
        unsubscribe: String,
    ) -> Result<UpstreamSubscription, SubscriptionCallbackError> {
        if let Some(rx) = self.upstream_subs.read().await.get(&key) {
            log::trace!("Found existing upstream subscription for {}", &subscribe);
            return Ok(rx.clone());
        }

        log::trace!("Create new upstream subscription for {}", &subscribe);

        let mut subscription = self
            .client
            .subscribe(&subscribe, params.clone(), &unsubscribe)
            .await
            .map_err(|e| SubscriptionCallbackError::Some(e.to_string()))?;

        let (tx, rx) = watch::channel(None);
        self.upstream_subs.write().await.insert(key, rx.clone());

        let client = self.client.clone();
        let upstream_subs = self.upstream_subs.clone();

        // this ticker acts like a waker to help cleanup subscriptions
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The first tick completes immediately
        interval.tick().await;

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    resp = subscription.next() => {
                        if tx.receiver_count() == 0 { break; }

                        interval.reset();

                        if let Some(Ok(value)) = resp {
                            if let Ok(message) = SubscriptionMessage::from_json(&value) {
                                if let Err(err) = tx.send(Some(message.clone())) {
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
                        if tx.receiver_count() <= 1 { break; }
                    }
                }
            }

            upstream_subs.write().await.remove(&key);
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

        let mut stream = self
            .get_upstream_subscription(
                key,
                request.subscribe.to_owned(),
                request.params.to_owned(),
                request.unsubscribe,
            )
            .await?;

        let sink = request.sink.accept().await?;

        // subscribe to upstream subscription
        tokio::spawn(async move {
            // this ticker acts like a waker to help cleanup sinks
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // The first tick completes immediately
            interval.tick().await;

            loop {
                tokio::select! {
                    resp = stream.changed() => {
                        if resp.is_err() { break; }
                        if sink.is_closed() { break; }

                        interval.reset();

                        let new_value = stream.borrow().to_owned();
                        if let Some(new_value) = new_value {
                            if (sink.send(new_value).await).is_err() { break; }
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
