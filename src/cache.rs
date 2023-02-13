use blake2::digest::Output;
use blake2::Digest;
use jsonrpsee::core::JsonValue;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
pub struct CacheKey<D: Digest>(pub Output<D>);

impl<D: Digest> CacheKey<D> {
    pub fn new(method: &String, params: &Vec<JsonValue>) -> Self {
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

#[derive(Clone)]
pub struct Cache<D: Digest> {
    map: Arc<Mutex<LruCache<CacheKey<D>, JsonValue>>>,
}

impl<D: Digest + 'static> Cache<D> {
    pub fn new(size: NonZeroUsize) -> Self {
        Self {
            map: Arc::new(Mutex::new(LruCache::new(size))),
        }
    }

    pub async fn get(&self, key: &CacheKey<D>) -> Option<JsonValue> {
        self.map.lock().await.get(key).cloned()
    }

    pub async fn put(&self, key: CacheKey<D>, value: JsonValue) {
        self.map.lock().await.put(key, value);
    }
}
