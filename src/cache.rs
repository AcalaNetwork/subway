use blake2::{digest::Output, Digest};
use jsonrpsee::core::JsonValue;
use std::num::NonZeroUsize;
use std::time::Duration;

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

pub type Cache<D> = moka::future::Cache<CacheKey<D>, JsonValue>;

pub fn new_cache<D: Digest + 'static>(size: NonZeroUsize, ttl: Option<Duration>) -> Cache<D> {
    let size = size.get();
    let mut builder = Cache::<D>::builder()
        .max_capacity(size as u64)
        .initial_capacity(size);

    if let Some(ttl) = ttl {
        builder = builder.time_to_live(ttl);
    }

    builder.build()
}
