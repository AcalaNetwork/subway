use async_trait::async_trait;
use jsonrpsee::core::{Error, JsonValue};
use schnellru::{ByLength, LruMap};
use std::sync::Mutex;

use super::{Middleware, NextFn, Request};

pub struct CacheMiddleware {
    whitelist: Vec<String>,
    map: Mutex<LruMap<(String, Vec<String>), JsonValue>>,
}

impl CacheMiddleware {
    pub fn new(whitelist: Vec<String>, length: u32) -> Self {
        Self {
            whitelist,
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
                if !self.whitelist.contains(method) {
                    return next(request).await;
                }

                let params: Vec<JsonValue> = params.parse()?;
                let key = (
                    method.clone(),
                    params.iter().map(|x| x.to_string()).collect::<Vec<_>>(),
                );
                if let Ok(mut map) = self.map.lock() {
                    if let Some(value) = map.get(&key) {
                        return Ok(value.clone());
                    }
                }
                let resp = next(request).await;
                if let Ok(ref value) = resp {
                    if let Ok(mut map) = self.map.lock() {
                        map.insert(key, value.clone());
                    }
                }
                resp
            }
            _ => next(request).await,
        }
    }
}
