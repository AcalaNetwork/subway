use opentelemetry::global::shutdown_tracer_provider;
use rand::{seq::SliceRandom, thread_rng};

use subway::{config, logger::enable_logger};

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

    // let mut endpoints = config.endpoints.clone();
    // endpoints.shuffle(&mut thread_rng());
    // let client = client::Client::new(endpoints)
    //     .await
    //     .map_err(anyhow::Error::msg)?;

    // let (addr, server) = server::start_server(&config, client).await?;

    // tracing::info!("Server running at {addr}");

    // server.stopped().await;

    // shutdown_tracer_provider();

    Ok(())
}
