use std::{
    any::{Any, TypeId},
    sync::Arc,
};

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
pub mod merge_subscription;
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
    client: client::Client,
    merge_subscription: merge_subscription::MergeSubscription,
    substrate_api: api::SubstrateApi,
    eth_api: api::EthApi,
    server: server::Server,
    event_bus: event_bus::EventBus,
}
