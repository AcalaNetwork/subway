use std::{any::TypeId, sync::Arc};

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::{
    extension::{Extension, ExtensionBuilder, ExtensionRegistry},
    utils::{TypeRegistry, TypeRegistryRef},
};

pub mod api;
pub mod cache;
pub mod client;
pub mod event_bus;
pub mod merge_subscriptions;
pub mod server;
pub mod telemetry;

macro_rules! define_all_extensions {
    (
        $(
            $ext_name:ident: $ext_type:ty
        ),* $(,)?
    ) => {
        #[derive(Deserialize, Debug)]
        pub struct ExtensionsConfig {
            $(
                #[serde(default)]
                $ext_name: Option<<$ext_type as Extension>::Config>,
            )*
        }

        #[async_trait]
        impl ExtensionBuilder for ExtensionsConfig {
            fn has<T: 'static>(&self) -> bool {
                match TypeId::of::<T>() {
                    $(
                        id if id == TypeId::of::<$ext_type>() => self.$ext_name.is_some(),
                    )*
                    _ => false,
                }
            }

            async fn build<T: 'static>(&self, registry: &TypeRegistryRef) -> Result<(), anyhow::Error> {
                match TypeId::of::<T>() {
                    $(
                        id if id == TypeId::of::<$ext_type>() => {
                            if let Some(config) = &self.$ext_name {
                                let ext = <$ext_type as Extension>::from_config(&config, &registry).await?;
                                let mut reg = registry.write().await;
                                if reg.has::<$ext_type>() {
                                    // some bad race condition???
                                    panic!("Extension already registered: {}", stringify!($ext_name));
                                }
                                reg.insert(ext);
                                Ok(())
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
                let ext_reg = ExtensionRegistry::new(reg.clone(), self);

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
    client: client::Client,
    merge_subscriptions: merge_subscriptions::MergeSubscriptions,
    substrate_api: api::SubstrateApi,
    eth_api: api::EthApi,
    server: server::Server,
    event_bus: event_bus::EventBus,
}
