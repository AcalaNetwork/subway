use opentelemetry::global::shutdown_tracer_provider;
use opentelemetry_datadog::new_pipeline;
use rand::seq::SliceRandom;
use rand::thread_rng;

use subway::client;
use subway::config;
use subway::enable_logger;
use subway::server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = match config::read_config() {
        Ok(config) => config,
        Err(e) => {
            return Err(anyhow::anyhow!(e));
        }
    };

    let tracer = if let Some(ref telemetry_config) = config.telemetry {
        if telemetry_config.enabled {
            let mut tracer = new_pipeline().with_service_name(
                telemetry_config
                    .service_name
                    .clone()
                    .unwrap_or_else(|| "subway".into()),
            );

            if let Some(ref agent_endpoint) = telemetry_config.agent_endpoint {
                tracer = tracer.with_agent_endpoint(agent_endpoint.clone());
            }

            let tracer = tracer.install_batch(opentelemetry::runtime::Tokio)?;

            Some(tracer)
        } else {
            None
        }
    } else {
        None
    };

    enable_logger(tracer);

    tracing::trace!("{:#?}", config);

    let mut endpoints = config.endpoints.clone();
    endpoints.shuffle(&mut thread_rng());
    let client = client::Client::new(endpoints)
        .await
        .map_err(anyhow::Error::msg)?;

    let (addr, server) = server::start_server(&config, client).await?;

    tracing::info!("Server running at {addr}");

    server.stopped().await;

    shutdown_tracer_provider();

    Ok(())
}
