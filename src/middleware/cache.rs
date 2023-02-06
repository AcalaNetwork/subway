use async_trait::async_trait;
use jsonrpsee::core::{Error, JsonValue};
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

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
        if request.skip_caching {
            return next(request).await;
        }

        let mut hasher = DefaultHasher::new();
        hasher.write(request.method.as_bytes());
        request
            .params
            .iter()
            .map(|x| x.to_string())
            .for_each(|x| hasher.write(x.as_bytes()));
        let key = hasher.finish();

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
