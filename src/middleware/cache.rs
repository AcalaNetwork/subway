use std::io::Write;

use async_trait::async_trait;
use blake2::{Blake2b512, Digest};
use jsonrpsee::core::{Error, JsonValue};

use super::{Middleware, NextFn};
use crate::{cache::Cache, middleware::call::CallRequest};

pub struct CacheMiddleware {
    cache: Cache,
}

impl CacheMiddleware {
    pub fn new(cache: Cache) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl Middleware<CallRequest, Result<JsonValue, Error>> for CacheMiddleware {
    async fn call(
        &self,
        request: CallRequest,
        next: NextFn<CallRequest, Result<JsonValue, Error>>,
    ) -> Result<JsonValue, Error> {
        let mut hasher = Blake2b512::new();
        hasher
            .write_all(request.method.as_bytes())
            .expect("should not fail");
        for p in &request.params {
            hasher
                .write_all(p.to_string().as_bytes())
                .expect("should not fail");
        }
        let key: [u8; 64] = hasher.finalize().into();

        if let Some(value) = self.cache.get(&key).await {
            return Ok(value);
        }

        let result = next(request).await;

        if let Ok(ref value) = result {
            self.cache.put(key, value.clone());
        }

        result
    }
}
