use std::{
    any::{Any, TypeId},
    sync::Arc,
};

use async_trait::async_trait;

use crate::utils::TypeRegistryRef;

#[async_trait]
pub trait Extension: Sized {
    type Config: serde::Deserialize<'static>;

    async fn from_config(
        config: &Self::Config,
        registry: &ExtensionRegistry,
    ) -> Result<Self, anyhow::Error>;
}

#[async_trait]
pub trait ExtensionBuilder {
    fn has(&self, type_id: TypeId) -> bool;
    async fn build(
        &self,
        type_id: TypeId,
        registry: &ExtensionRegistry,
    ) -> anyhow::Result<Arc<dyn Any + Send + Sync>>;
}

pub struct ExtensionRegistry {
    pub registry: TypeRegistryRef,
    builder: Arc<dyn ExtensionBuilder + Send + Sync>,
}

impl ExtensionRegistry {
    pub fn new(
        registry: TypeRegistryRef,
        builder: Arc<dyn ExtensionBuilder + Send + Sync>,
    ) -> Self {
        Self { registry, builder }
    }

    pub async fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let reg = self.registry.read().await;

        let ext = reg.get::<T>();

        if ext.is_none() && self.builder.has(TypeId::of::<T>()) {
            drop(reg);
            let ext = self
                .builder
                .build(TypeId::of::<T>(), &self)
                .await
                .expect("Failed to build extension");
            self.registry.write().await.insert_raw(ext);
            let reg = self.registry.read().await;
            let ext = reg.get::<T>();
            assert!(ext.is_some());
            ext
        } else {
            ext
        }
    }
}
