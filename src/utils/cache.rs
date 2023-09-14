use blake2::{digest::Output, Digest};
use futures::future::BoxFuture;
use jsonrpsee::core::JsonValue;
use jsonrpsee::types::ErrorObjectOwned;
use std::num::NonZeroUsize;
use std::time::Duration;
use tokio::sync::watch;

#[derive(Debug)]
pub struct CacheKey<D: Digest>(pub Output<D>);

impl<D: Digest> Clone for CacheKey<D> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<D: Digest> CacheKey<D> {
    pub fn new(method: &String, params: &[JsonValue]) -> Self {
        let mut hasher = D::new();
        hasher.update(method.as_bytes());
        for p in params {
            hasher.update(p.to_string().as_bytes());
        }

        Self(hasher.finalize())
    }
}

impl<D: Digest> PartialEq for CacheKey<D> {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_slice() == other.0.as_slice()
    }
}

impl<D: Digest> Eq for CacheKey<D> {}

impl<D: Digest> std::hash::Hash for CacheKey<D> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.as_slice().hash(state);
    }
}

#[derive(Clone, Debug)]
pub enum CacheValue {
    Pending(watch::Receiver<Option<Result<JsonValue, ErrorObjectOwned>>>),
    Value(JsonValue),
}

#[derive(Clone)]
pub struct Cache<D: Digest> {
    cache: moka::future::Cache<CacheKey<D>, CacheValue>,
}

impl<D: Digest + 'static> Cache<D> {
    pub fn new(size: NonZeroUsize, ttl: Option<Duration>) -> Self {
        let size = size.get();
        let mut builder = moka::future::Cache::<CacheKey<D>, CacheValue>::builder()
            .max_capacity(size as u64)
            .initial_capacity(size);

        if let Some(duration) = ttl {
            builder = builder.time_to_live(duration);
        }

        let cache = builder.build();

        Self { cache }
    }

    pub async fn get(&self, key: &CacheKey<D>) -> Option<JsonValue> {
        match self.cache.get(key) {
            Some(CacheValue::Value(value)) => Some(value),
            Some(CacheValue::Pending(mut rx)) => {
                let value = rx.borrow();
                if value.is_some() {
                    return value.clone().unwrap().ok();
                }
                drop(value);
                let _ = rx.changed().await;
                let value = rx.borrow();
                if value.is_some() {
                    return value.clone().unwrap().ok();
                } else {
                    tracing::error!("Cache: Unreachable code");
                    return None;
                }
            }
            None => None,
        }
    }

    pub async fn insert(&self, key: CacheKey<D>, value: JsonValue) {
        self.cache.insert(key, CacheValue::Value(value)).await;
    }

    pub async fn get_or_insert_with<F>(
        &self,
        key: &CacheKey<D>,
        f: F,
    ) -> Result<JsonValue, ErrorObjectOwned>
    where
        F: FnOnce() -> BoxFuture<'static, Result<JsonValue, ErrorObjectOwned>>,
    {
        match self.cache.get(key) {
            Some(CacheValue::Value(value)) => Ok(value),
            Some(CacheValue::Pending(rx)) => rx.borrow().clone().unwrap(),
            None => {
                let (tx, rx) = watch::channel(None);
                self.cache
                    .insert(key.clone(), CacheValue::Pending(rx))
                    .await;
                let value = f().await;
                let _ = tx.send(Some(value.clone()));
                match &value {
                    Ok(value) => {
                        self.cache
                            .insert(key.clone(), CacheValue::Value(value.clone()))
                            .await;
                    }
                    Err(_) => {
                        self.cache.remove(key).await;
                    }
                };
                value
            }
        }
    }

    pub async fn remove(&self, key: &CacheKey<D>) {
        self.cache.remove(key).await;
    }

    pub fn sync(&self) {
        use moka::future::ConcurrentCacheExt;

        self.cache.sync();
    }
}
