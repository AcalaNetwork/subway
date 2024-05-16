mod rpc_metrics;

use super::{Extension, ExtensionRegistry};
use async_trait::async_trait;
use serde::Deserialize;
use std::iter;
use std::net::SocketAddr;
use substrate_prometheus_endpoint::init_prometheus;
use substrate_prometheus_endpoint::Registry;
use tokio::task::JoinHandle;

use crate::utils::TypeRegistryRef;
pub use rpc_metrics::RpcMetrics;

pub async fn get_rpc_metrics(registry: &TypeRegistryRef) -> RpcMetrics {
    let prometheus = registry.read().await.get::<Prometheus>();

    match prometheus {
        None => RpcMetrics::noop(),
        Some(prom) => prom.rpc_metrics(),
    }
}

pub struct Prometheus {
    registry: Registry,
    rpc_metrics: RpcMetrics,
    exporter_task: JoinHandle<()>,
}

impl Drop for Prometheus {
    fn drop(&mut self) {
        self.exporter_task.abort();
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct PrometheusConfig {
    pub port: u16,
    pub listen_address: String,
    pub prefix: Option<String>,
    pub chain_label: Option<String>,
}

#[async_trait]
impl Extension for Prometheus {
    type Config = PrometheusConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config.clone()))
    }
}

impl Prometheus {
    pub fn new(config: PrometheusConfig) -> Self {
        let labels = config
            .chain_label
            .clone()
            .map(|l| iter::once(("chain".to_string(), l.clone())).collect());

        // make sure the prefix is not an Option of Some empty string
        let prefix = match config.prefix {
            Some(p) if p.is_empty() => None,
            p => p,
        };
        let registry = Registry::new_custom(prefix, labels)
            .expect("It can't fail, we make sure the `prefix` is either `None` or `Some` of non-empty string");
        let rpc_metrics = RpcMetrics::new(&registry);

        let exporter_task = start_prometheus_exporter(registry.clone(), config.port, config.listen_address);
        Self {
            registry,
            exporter_task,
            rpc_metrics,
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn rpc_metrics(&self) -> RpcMetrics {
        self.rpc_metrics.clone()
    }
}

fn start_prometheus_exporter(registry: Registry, port: u16, listen_address: String) -> JoinHandle<()> {
    let address = listen_address.parse().expect("Invalid prometheus listen address");
    let addr = SocketAddr::new(address, port);

    tokio::spawn(async move {
        init_prometheus(addr, registry).await.expect("Init prometeus failed");
    })
}
