use std::env;

use opentelemetry::{global, sdk::trace::Tracer, trace::TraceError};

use crate::config::{TelemetryOptions, TelemetryProvider};

pub fn setup_telemetry(options: &Option<TelemetryOptions>) -> Result<Option<Tracer>, TraceError> {
    let Some(options) = options else {
        return Ok(None);
    };

    global::set_error_handler(|e| {
        log::warn!("OpenTelemetry error: {}", e);
    })
    .expect("failed to set OpenTelemetry error handler");

    let service_name = options
        .service_name
        .clone()
        .unwrap_or_else(|| "subway".into());

    let tracer = match options.provider {
        TelemetryProvider::Jaeger => {
            let mut tracer =
                opentelemetry_jaeger::new_agent_pipeline().with_service_name(service_name);

            if let Some(ref agent_endpoint) = options.agent_endpoint {
                tracer = tracer.with_endpoint(agent_endpoint.clone());
            }

            let tracer = tracer.install_batch(opentelemetry::runtime::Tokio)?;

            Some(tracer)
        }
        TelemetryProvider::Datadog => {
            let mut tracer = opentelemetry_datadog::new_pipeline()
                .with_service_name(service_name)
                .with_version(std::env::var("SUBWAY_VERSION").unwrap_or("dev".into()));

            let agent_endpoint = env::var("DATADOG_AGENT_ENDPOINT")
                .ok()
                .or_else(|| options.agent_endpoint.clone());
            if let Some(agent_endpoint) = agent_endpoint {
                tracer = tracer.with_agent_endpoint(agent_endpoint);
            }

            let tracer = tracer.install_batch(opentelemetry::runtime::Tokio)?;

            Some(tracer)
        }
        TelemetryProvider::None => None,
    };

    Ok(tracer)
}
