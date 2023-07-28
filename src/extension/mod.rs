use std::sync::Arc;

use async_trait::async_trait;

use crate::utils::TypeRegistryRef;

#[async_trait]
pub trait Extension: Sized {
    type Config: serde::Deserialize<'static>;

    async fn from_config(
        config: &Self::Config,
        registry: &TypeRegistryRef,
    ) -> Result<Self, anyhow::Error>;
}

#[async_trait]
pub trait ExtensionBuilder {
    fn has<T: 'static>(&self) -> bool;
    async fn build<T: 'static>(&self, registry: &TypeRegistryRef) -> Result<(), anyhow::Error>;
}

pub struct ExtensionRegistry<TBuilder> {
    registry: TypeRegistryRef,
    builder: TBuilder,
}

impl<TBuilder: ExtensionBuilder> ExtensionRegistry<TBuilder> {
    pub fn new(registry: TypeRegistryRef, builder: TBuilder) -> Self {
        Self { registry, builder }
    }

    pub async fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let reg = self.registry.read().await;

        let ext = reg.get::<T>();

        if ext.is_none() && self.builder.has::<T>() {
            drop(reg);
            self.builder
                .build::<T>(&self.registry)
                .await
                .expect("Failed to build extension");
            let reg = self.registry.read().await;
            reg.get::<T>()
        } else {
            ext
        }
    }
}
