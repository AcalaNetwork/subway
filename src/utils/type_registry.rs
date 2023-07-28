// taken from https://github.com/hyperium/http/blob/master/src/extensions.rs.

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
    sync::Arc,
};
use tokio::sync::RwLock;

pub type AnyMap = HashMap<TypeId, Arc<dyn Any + Send + Sync>, BuildHasherDefault<IdHasher>>;

/// With TypeIds as keys, there's no need to hash them. They are already hashes
/// themselves, coming from the compiler. The IdHasher holds the u64 of
/// the TypeId, and then returns it, instead of doing any bit fiddling.
#[derive(Default, Debug)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("TypeId calls write_u64");
    }

    #[inline]
    fn write_u64(&mut self, id: u64) {
        self.0 = id;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub struct TypeRegistry {
    map: AnyMap,
}

impl TypeRegistry {
    pub fn new() -> TypeRegistry {
        TypeRegistry {
            map: AnyMap::default(),
        }
    }

    pub fn has<T: 'static>(&self) -> bool {
        self.map.contains_key(&TypeId::of::<T>())
    }

    pub fn insert<T: Send + Sync + 'static>(&mut self, val: T) {
        self.map.insert(TypeId::of::<T>(), Arc::new(val));
    }

    pub fn get<T: 'static>(&self) -> Option<Arc<T>> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref())
            .map(|boxed: &Arc<T>| boxed.clone())
    }

    pub fn remove<T: Send + Sync + 'static>(&mut self) {
        self.map.remove(&TypeId::of::<T>());
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

pub type TypeRegistryRef = Arc<RwLock<TypeRegistry>>;
