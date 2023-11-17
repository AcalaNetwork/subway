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
        match self.cache.get(key).await {
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
                    value.clone().unwrap().ok()
                } else {
                    tracing::error!("Cache: Unreachable code");
                    None
                }
            }
            None => None,
        }
    }

    pub async fn insert(&self, key: CacheKey<D>, value: JsonValue) {
        self.cache.insert(key, CacheValue::Value(value)).await;
    }

    pub async fn get_or_insert_with<F>(&self, key: CacheKey<D>, f: F) -> Result<JsonValue, ErrorObjectOwned>
    where
        F: FnOnce() -> BoxFuture<'static, Result<JsonValue, ErrorObjectOwned>>,
    {
        let fetch = || async {
            let (tx, rx) = watch::channel(None);
            self.cache.insert(key.clone(), CacheValue::Pending(rx)).await;
            let value = f().await;
            let _ = tx.send(Some(value.clone()));
            match &value {
                Ok(value) => {
                    self.cache.insert(key.clone(), CacheValue::Value(value.clone())).await;
                }
                Err(_) => {
                    self.cache.remove(&key).await;
                }
            };
            value
        };

        match self.cache.get(&key).await {
            Some(CacheValue::Value(value)) => Ok(value),
            Some(CacheValue::Pending(mut rx)) => {
                {
                    // limit the scope of value
                    let value = rx.borrow();
                    if value.is_some() {
                        return value.clone().unwrap();
                    }
                }

                let _ = rx.changed().await;

                {
                    // limit the scope of value
                    let value = rx.borrow();
                    if let Some(value) = &*value {
                        return value.clone();
                    }
                }

                // this only happens when initial fetch request got canceled for some reason
                // in that case we need to fetch again
                fetch().await
            }
            None => fetch().await,
        }
    }

    pub async fn remove(&self, key: &CacheKey<D>) {
        self.cache.remove(key).await;
    }

    pub async fn sync(&self) {
        self.cache.run_pending_tasks().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt as _;
    use jsonrpsee::types::error::reject_too_big_request;
    use serde_json::json;

    #[tokio::test]
    async fn get_insert_remove() {
        let cache = Cache::<blake2::Blake2b512>::new(NonZeroUsize::new(1).unwrap(), None);

        let key = CacheKey::<blake2::Blake2b512>::new(&"key".to_string(), &[]);

        assert_eq!(cache.get(&key).await, None);

        cache.insert(key.clone(), json!("value")).await;

        assert_eq!(cache.get(&key).await, Some(json!("value")));

        cache.remove(&key).await;

        assert_eq!(cache.get(&key).await, None);
    }

    #[tokio::test]
    async fn get_or_insert_with_basic() {
        let cache = Cache::<blake2::Blake2b512>::new(NonZeroUsize::new(1).unwrap(), None);

        let key = CacheKey::<blake2::Blake2b512>::new(&"key".to_string(), &[]);

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let cache2 = cache.clone();
        let key2 = key.clone();
        let h1 = tokio::spawn(async move {
            let value = cache2
                .get_or_insert_with(key2.clone(), || {
                    async move {
                        let _ = rx.await;
                        Ok(json!("value"))
                    }
                    .boxed()
                })
                .await;
            assert_eq!(value, Ok(json!("value")));
        });

        tokio::task::yield_now().await;

        let cache2 = cache.clone();
        let key2 = key.clone();
        let h2 = tokio::spawn(async move {
            let value = cache2
                .get_or_insert_with(key2, || {
                    async {
                        panic!();
                    }
                    .boxed()
                })
                .await;
            assert_eq!(value, Ok(json!("value")));
        });

        tokio::task::yield_now().await;

        tx.send(()).unwrap();

        h1.await.unwrap();
        h2.await.unwrap();

        assert_eq!(cache.get(&key).await, Some(json!("value")));
    }

    #[tokio::test]
    async fn get_or_insert_with_handle_canceled_request() {
        let cache = Cache::<blake2::Blake2b512>::new(NonZeroUsize::new(1).unwrap(), None);

        let key = CacheKey::<blake2::Blake2b512>::new(&"key".to_string(), &[]);

        let (_tx, rx) = tokio::sync::oneshot::channel::<()>();

        let cache2 = cache.clone();
        let key2 = key.clone();
        let h1 = tokio::spawn(async move {
            let _ = cache2
                .get_or_insert_with(key2.clone(), || {
                    async move {
                        let _ = rx.await;
                        panic!();
                    }
                    .boxed()
                })
                .await;
            unreachable!();
        });

        tokio::task::yield_now().await;

        let cache2 = cache.clone();
        let key2 = key.clone();
        let h2 = tokio::spawn(async move {
            let value = cache2
                .get_or_insert_with(key2, || async { Ok(json!("value")) }.boxed())
                .await;
            assert_eq!(value, Ok(json!("value")));
        });

        tokio::task::yield_now().await;

        h1.abort(); // first request failed for whatever reason

        h1.await.unwrap_err();
        h2.await.unwrap(); // second request should still work

        assert_eq!(cache.get(&key).await, Some(json!("value")));
    }

    #[tokio::test]
    async fn get_or_insert_with_error() {
        let cache = Cache::<blake2::Blake2b512>::new(NonZeroUsize::new(1).unwrap(), None);

        let key = CacheKey::<blake2::Blake2b512>::new(&"key".to_string(), &[]);

        let value = cache
            .get_or_insert_with(key.clone(), || async move { Err(reject_too_big_request(100)) }.boxed())
            .await;
        assert_eq!(value, Err(reject_too_big_request(100)));

        assert_eq!(cache.get(&key).await, None);
    }
}
