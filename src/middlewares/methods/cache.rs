use std::num::NonZeroUsize;

use async_trait::async_trait;
use blake2::Blake2b512;
use futures::FutureExt as _;
use jsonrpsee::{core::JsonValue, types::ErrorObjectOwned};
use opentelemetry::trace::FutureExt;

use crate::{
    extensions::cache::Cache as CacheExtension,
    middleware::{Middleware, MiddlewareBuilder, NextFn, RpcMethod},
    middlewares::{CallRequest, CallResult},
    utils::{telemetry, Cache, CacheKey, TypeRegistry, TypeRegistryRef},
};

pub struct BypassCache(pub bool);

pub struct CacheMiddleware {
    cache: Cache<Blake2b512>,
}

impl CacheMiddleware {
    pub fn new(cache: Cache<Blake2b512>) -> Self {
        Self { cache }
    }
}

#[async_trait]
impl MiddlewareBuilder<RpcMethod, CallRequest, CallResult> for CacheMiddleware {
    async fn build(
        method: &RpcMethod,
        extensions: &TypeRegistryRef,
    ) -> Option<Box<dyn Middleware<CallRequest, CallResult>>> {
        let params = method.cache.as_ref()?;
        if params.size == Some(0) {
            return None;
        }
        let cache_ext = extensions
            .read()
            .await
            .get::<CacheExtension>()
            .expect("Cache extension not found");

        let size =
            NonZeroUsize::new(params.size.unwrap_or(cache_ext.config.default_size) as usize)?;

        let cache = Cache::new(
            size,
            params
                .ttl_seconds
                .map(|s| std::time::Duration::from_secs(s as u64)),
        );

        Some(Box::new(Self::new(cache)))
    }
}

const TRACER: telemetry::Tracer = telemetry::Tracer::new("cache-middleware");

#[async_trait]
impl Middleware<CallRequest, Result<JsonValue, ErrorObjectOwned>> for CacheMiddleware {
    async fn call(
        &self,
        request: CallRequest,
        context: TypeRegistry,
        next: NextFn<CallRequest, Result<JsonValue, ErrorObjectOwned>>,
    ) -> Result<JsonValue, ErrorObjectOwned> {
        async move {
            let bypass_cache = context.get::<BypassCache>().map(|v| v.0).unwrap_or(false);
            if bypass_cache {
                return next(request, context).await;
            }

            let key = CacheKey::<Blake2b512>::new(&request.method, &request.params);

            let result = self
                .cache
                .get_or_insert_with(key.clone(), || next(request, context).boxed())
                .await;

            if let Ok(ref value) = result {
                // avoid caching null value because it usually means data not available
                // but it could be available in the future
                if value.is_null() {
                    self.cache.remove(&key).await;
                }
            }

            result
        }
        .with_context(TRACER.context("call"))
        .await
    }
}

#[cfg(test)]
mod tests {
    use futures::FutureExt;
    use serde_json::json;
    use std::num::NonZeroUsize;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn handle_ok_resp() {
        let cache = Cache::new(NonZeroUsize::try_from(1).unwrap(), None);
        let middleware = CacheMiddleware::new(cache.clone());

        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Default::default(),
                Box::new(move |_, _| async move { Ok(json!(1)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(1));

        // cache hit
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Default::default(),
                Box::new(move |_, _| async move { panic!() }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(1));

        // cache miss with different params
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(22)]),
                Default::default(),
                Box::new(move |_, _| async move { Ok(json!(2)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(2));

        // cache miss with different method
        let res = middleware
            .call(
                CallRequest::new("test2", vec![json!(22)]),
                Default::default(),
                Box::new(move |_, _| async move { Ok(json!(3)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(3));

        // cache hit and update prune priority
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Default::default(),
                Box::new(move |_, _| async move { panic!() }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(1));

        // cache override oldest entry
        let res = middleware
            .call(
                CallRequest::new("test2", vec![json!(33)]),
                Default::default(),
                Box::new(move |_, _| async move { Ok(json!(4)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(4));

        // ensure cache is fully updated
        cache.sync();

        // cache miss due to entry pruned
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(22)]),
                Default::default(),
                Box::new(move |_, _| async move { Ok(json!(5)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(5));
    }

    #[tokio::test]
    async fn should_not_cache_null() {
        let middleware = CacheMiddleware::new(Cache::new(NonZeroUsize::try_from(3).unwrap(), None));

        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Default::default(),
                Box::new(move |_, _| async move { Ok(JsonValue::Null) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), JsonValue::Null);

        // wait for cache write
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;

        // should not be cached
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Default::default(),
                Box::new(move |_, _| async move { Ok(json!(2)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(2));
    }

    #[tokio::test]
    async fn cache_ttl_works() {
        let middleware = CacheMiddleware::new(Cache::new(
            NonZeroUsize::new(1).unwrap(),
            Some(Duration::from_millis(10)),
        ));

        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Default::default(),
                Box::new(move |_, _| async move { Ok(json!(1)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(1));

        // wait for cache write
        tokio::time::sleep(Duration::from_millis(1)).await;

        // cache hit
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Default::default(),
                Box::new(move |_, _| async move { panic!() }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(1));

        // wait for cache to expire
        tokio::time::sleep(Duration::from_millis(10)).await;

        // cache miss
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Default::default(),
                Box::new(move |_, _| async move { Ok(json!(2)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(2));
    }

    #[tokio::test]
    async fn bypass_cache() {
        let middleware = CacheMiddleware::new(Cache::new(NonZeroUsize::try_from(3).unwrap(), None));

        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                Default::default(),
                Box::new(move |_, _| async move { Ok(json!(1)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(1));

        // wait for cache write
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;

        let mut context = TypeRegistry::new();
        context.insert(BypassCache(true));

        // bypass cache
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                context,
                Box::new(move |_, _| async move { Ok(json!(2)) }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(2));

        let mut context = TypeRegistry::new();
        context.insert(BypassCache(false));

        // do not bypass cache, value from bypass cache request is not cached
        let res = middleware
            .call(
                CallRequest::new("test", vec![json!(11)]),
                context,
                Box::new(move |_, _| async move { panic!() }.boxed()),
            )
            .await;
        assert_eq!(res.unwrap(), json!(1));
    }

    #[tokio::test]
    async fn avoid_repeated_requests() {
        let middleware = CacheMiddleware::new(Cache::new(NonZeroUsize::try_from(3).unwrap(), None));

        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let res = middleware.call(
            CallRequest::new("test", vec![json!(11)]),
            Default::default(),
            Box::new(move |_, _| async move { Ok(rx.recv().await.unwrap()) }.boxed()),
        );

        let res2 = middleware.call(
            CallRequest::new("test", vec![json!(11)]),
            Default::default(),
            Box::new(move |_, _| async move { panic!() }.boxed()),
        );

        tx.send(json!(1)).await.unwrap();

        let res = res.await;
        let res2 = res2.await;

        assert_eq!(res.unwrap(), json!(1));
        assert_eq!(res2.unwrap(), json!(1));
    }
}
