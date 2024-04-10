use std::env;

use async_trait::async_trait;
use opentelemetry::{global, trace::TraceError};
use opentelemetry_sdk::trace::Tracer;
use serde::Deserialize;

use super::{Extension, ExtensionRegistry};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryProvider {
    None,
    Datadog,
    Jaeger,
}

#[derive(Deserialize, Debug)]
pub struct TelemetryConfig {
    pub provider: TelemetryProvider,
    #[serde(default)]
    pub service_name: Option<String>,
    #[serde(default)]
    pub agent_endpoint: Option<String>,
}

pub struct Telemetry {
    tracer: Option<Tracer>,
}

#[async_trait]
impl Extension for Telemetry {
    type Config = TelemetryConfig;

    async fn from_config(config: &Self::Config, _registry: &ExtensionRegistry) -> Result<Self, anyhow::Error> {
        Ok(Self::new(config)?)
    }
}

impl Telemetry {
    pub fn new(config: &TelemetryConfig) -> Result<Self, anyhow::Error> {
        let tracer = setup_telemetry(config)?;

        Ok(Self { tracer })
    }

    pub fn tracer(&self) -> Option<&Tracer> {
        self.tracer.as_ref()
    }
}

pub fn setup_telemetry(options: &TelemetryConfig) -> Result<Option<Tracer>, TraceError> {
    global::set_error_handler(|e| {
        tracing::warn!("OpenTelemetry error: {}", e);
    })
    .expect("failed to set OpenTelemetry error handler");

    let service_name = options.service_name.clone().unwrap_or_else(|| "subway".into());

    let tracer = match options.provider {
        TelemetryProvider::Jaeger => {
            let mut tracer = opentelemetry_jaeger::new_agent_pipeline().with_service_name(service_name);

            if let Some(ref agent_endpoint) = options.agent_endpoint {
                tracer = tracer.with_endpoint(agent_endpoint.clone());
            }

            let tracer = tracer.install_batch(opentelemetry_sdk::runtime::Tokio)?;

            Some(tracer)
        }
        TelemetryProvider::Datadog => {
            let mut tracer = opentelemetry_datadog::new_pipeline()
                .with_service_name(service_name)
                .with_version(option_env!("SUBWAY_VERSION").unwrap_or("dev"));

            let agent_endpoint = env::var("DATADOG_AGENT_ENDPOINT")
                .ok()
                .or_else(|| options.agent_endpoint.clone());
            if let Some(agent_endpoint) = agent_endpoint {
                tracer = tracer.with_agent_endpoint(agent_endpoint);
            }

            let tracer = tracer.install_batch(opentelemetry_sdk::runtime::Tokio)?;

            Some(tracer)
        }
        TelemetryProvider::None => None,
    };

    Ok(tracer)
}
