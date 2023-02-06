use jsonrpsee::core::JsonValue;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct Cache {
    map: Arc<Mutex<LruCache<Vec<String>, JsonValue>>>,
}

impl Cache {
    pub fn new(size: NonZeroUsize) -> Self {
        Self {
            map: Arc::new(Mutex::new(LruCache::new(size))),
        }
    }

    pub async fn get(&self, key: &Vec<String>) -> Option<JsonValue> {
        self.map.lock().await.get(key).cloned()
    }

    pub fn put(&self, key: Vec<String>, value: JsonValue) {
        let map = self.map.clone();
        tokio::spawn(async move {
            let _ = map.lock().await.put(key, value);
        });
    }
}
