use async_trait::async_trait;
use jsonrpsee::core::{Error, JsonValue};
use schnellru::{ByLength, LruMap};
use tokio::sync::Mutex;

use super::{Middleware, NextFn, Request};

pub struct CacheMiddleware {
    map: Mutex<LruMap<(String, Vec<String>), JsonValue>>,
}

impl CacheMiddleware {
    pub fn new(length: u32) -> Self {
        Self {
            map: Mutex::new(LruMap::new(ByLength::new(length))),
        }
    }
}

#[async_trait]
impl Middleware for CacheMiddleware {
    async fn call(
        &self,
        request: Request,
        next: NextFn<Request, Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        match &request {
            Request::Call { method, params } => {
                let params: Vec<JsonValue> = params.parse()?;
                let key = (
                    method.clone(),
                    params.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
                );
                let mut map = self.map.lock().await;
                if let Some(value) = map.get(&key) {
                    return Ok(value.clone());
                }
                let resp = next(request).await;
                if let Ok(ref value) = resp {
                    let mut map = self.map.lock().await;
                    map.insert(key, value.clone());
                }
                resp
            }
            _ => next(request).await,
        }
    }
}
