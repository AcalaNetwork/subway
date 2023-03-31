use async_trait::async_trait;
use blake2::Blake2b512;
use jsonrpsee::core::{Error, JsonValue};

use super::{Middleware, NextFn};
use crate::{
    cache::{Cache, CacheKey},
    middleware::call::CallRequest,
};

pub struct CacheMiddleware {
    cache: Cache<Blake2b512>,
}

impl CacheMiddleware {
    pub fn new(cache: Cache<Blake2b512>) -> Self {
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
        if request.extra.bypass_cache {
            return next(request).await;
        }

        let key = CacheKey::<Blake2b512>::new(&request.method, &request.params);

        if let Some(value) = self.cache.get(&key) {
            return Ok(value);
        }

        let result = next(request).await;

        if let Ok(ref value) = result {
            let cache = self.cache.clone();
            let value = value.clone();
            tokio::spawn(async move {
                cache.insert(key, value).await;
            });
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn handle_ok_resp() {
        let middleware = CacheMiddleware::new(Cache::new(3));

        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Box::new(move |_| async move { Ok(json!(1)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(1));

        // wait for cache write
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;

        // cache hit
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Box::new(move |_| async move { panic!() }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(1));

        // cache miss with different params
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(22)]),
                Box::new(move |_| async move { Ok(json!(2)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(2));

        // cache miss with different method
        let res = middleware
            .call(
                CallRequest::new("test2", vec![json!(22)]),
                Box::new(move |_| async move { Ok(json!(3)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(3));

        // cache hit and update prune priority
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Box::new(move |_| async move { panic!() }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(1));

        // cache override oldest entry
        let res = middleware
            .call(
                CallRequest::new("test2", vec![json!(33)]),
                Box::new(move |_| async move { Ok(json!(4)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(4));

        // cache miss due to entry pruned
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(22)]),
                Box::new(move |_| async move { Ok(json!(5)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(5));
    }
}
