use opentelemetry::global::shutdown_tracer_provider;

use subway::{config, logger::enable_logger, server};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = match config::read_config() {
        Ok(config) => config,
        Err(e) => {
            return Err(anyhow::anyhow!(e));
        }
    };

    enable_logger();

    tracing::trace!("{:#?}", config);

    let (addr, server) = server::start_server(config).await?;

    tracing::info!("Server running at {addr}");

    server.stopped().await;

    shutdown_tracer_provider();

    Ok(())
}
