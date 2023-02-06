use jsonrpsee::core::JsonValue;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct Cache {
    map: Arc<Mutex<LruCache<u64, JsonValue>>>,
}

impl Cache {
    pub fn new(size: NonZeroUsize) -> Self {
        Self {
            map: Arc::new(Mutex::new(LruCache::new(size))),
        }
    }

    pub async fn get(&self, key: &u64) -> Option<JsonValue> {
        self.map.lock().await.get(key).cloned()
    }

    pub fn put(&self, key: u64, value: JsonValue) {
        let map = self.map.clone();
        tokio::spawn(async move {
            let _ = map.lock().await.put(key, value);
        });
    }
}
