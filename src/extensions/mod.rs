use async_trait::async_trait;
use serde::Deserialize;
use std::{
    any::{Any, TypeId},
    sync::Arc,
};
use tokio::sync::RwLock;

use crate::utils::{TypeRegistry, TypeRegistryRef};

pub mod api;
pub mod cache;
pub mod client;
pub mod event_bus;
pub mod merge_subscription;
pub mod prometheus;
pub mod rate_limit;
pub mod server;
pub mod telemetry;
pub mod validator;

#[async_trait]
pub trait Extension: Sized {
    type Config: serde::Deserialize<'static>;

    async fn from_config(config: &Self::Config, registry: &ExtensionRegistry) -> Result<Self, anyhow::Error>;
}

#[async_trait]
pub trait ExtensionBuilder {
    fn has(&self, type_id: TypeId) -> bool;
    async fn build(&self, type_id: TypeId, registry: &ExtensionRegistry) -> anyhow::Result<Arc<dyn Any + Send + Sync>>;
}

/// ExtensionRegistry is a struct that holds a registry of types and an extension builder.
/// It allows to get an instance of a type from the registry or build it using the extension builder.
pub struct ExtensionRegistry {
    pub registry: TypeRegistryRef,
    builder: Arc<dyn ExtensionBuilder + Send + Sync>,
}

impl ExtensionRegistry {
    /// Creates a new ExtensionRegistry instance with the given registry and builder.
    pub fn new(registry: TypeRegistryRef, builder: Arc<dyn ExtensionBuilder + Send + Sync>) -> Self {
        Self { registry, builder }
    }

    /// Gets an instance of the given type from the registry or builds it using the extension builder.
    pub async fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let reg = self.registry.read().await;

        let ext = reg.get::<T>();

        if ext.is_none() && self.builder.has(TypeId::of::<T>()) {
            drop(reg);
            let ext = self
                .builder
                .build(TypeId::of::<T>(), self)
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

// This macro generates the ExtensionsConfig and implements ExtensionBuilder so extensions can be built from config.
macro_rules! define_all_extensions {
    (
        $(
            $(#[$attr:meta])* $ext_name:ident: $ext_type:ty
        ),* $(,)?
    ) => {
        use garde::Validate;
        #[derive(Deserialize, Debug, Validate, Default)]
        #[garde(allow_unvalidated)]
        pub struct ExtensionsConfig {
            $(
                $(#[$attr])*
                pub $ext_name: Option<<$ext_type as Extension>::Config>,
            )*
        }

        #[async_trait]
        impl ExtensionBuilder for ExtensionsConfig {
            fn has(&self, type_id: TypeId) -> bool {
                match type_id {
                    $(
                        id if id == TypeId::of::<$ext_type>() => self.$ext_name.is_some(),
                    )*
                    _ => false,
                }
            }

            async fn build(&self, type_id: TypeId, registry: &ExtensionRegistry) -> anyhow::Result<Arc<dyn Any + Send + Sync>> {
                match type_id {
                    $(
                        id if id == TypeId::of::<$ext_type>() => {
                            if let Some(config) = &self.$ext_name {
                                let ext = <$ext_type as Extension>::from_config(&config, &registry).await?;
                                Ok(Arc::new(ext))
                            } else {
                                anyhow::bail!("No config for extension: {}", stringify!($ext_name));
                            }
                        }
                    )*
                    id => {
                        anyhow::bail!("Unknown extension: {:?}", id);
                    }
                }
            }
        }

        impl ExtensionsConfig {
            pub async fn create_registry(self) -> Result<TypeRegistryRef, anyhow::Error> {
                let reg = Arc::new(RwLock::new(TypeRegistry::new()));
                let ext_reg = ExtensionRegistry::new(reg.clone(), Arc::new(self));

                // ensure all the extensions are created
                $(
                    let _ = ext_reg.get::<$ext_type>().await;
                )*

                Ok(reg)
            }
        }
    };
}

define_all_extensions! {
    telemetry: telemetry::Telemetry,
    cache: cache::Cache,
    #[garde(dive)]
    client: client::Client,
    merge_subscription: merge_subscription::MergeSubscription,
    substrate_api: api::SubstrateApi,
    eth_api: api::EthApi,
    server: server::SubwayServerBuilder,
    event_bus: event_bus::EventBus,
    rate_limit: rate_limit::RateLimitBuilder,
    prometheus: prometheus::Prometheus,
    validator: validator::Validator,
}
