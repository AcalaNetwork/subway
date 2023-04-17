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
    enable_logger();

    let config = match config::read_config() {
        Ok(config) => config,
        Err(e) => {
            return Err(anyhow::anyhow!(e));
        }
    };

    tracing::trace!("{:#?}", config);

    if let Some(ref telemetry_config) = config.telemetry {
        let mut tracer = new_pipeline().with_service_name(
            telemetry_config
                .service_name
                .clone()
                .unwrap_or_else(|| "subway".into()),
        );

        if let Some(ref agent_endpoint) = telemetry_config.agent_endpoint {
            tracer = tracer.with_agent_endpoint(agent_endpoint.clone());
        }

        tracer.install_batch(opentelemetry::runtime::Tokio)?;
    };

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
