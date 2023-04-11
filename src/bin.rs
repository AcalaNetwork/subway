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

    let mut endpoints = config.endpoints.clone();
    endpoints.shuffle(&mut thread_rng());
    let client = client::Client::new(endpoints)
        .await
        .map_err(anyhow::Error::msg)?;

    let (addr, server) = server::start_server(&config, client).await?;

    tracing::info!("Server running at {addr}");

    server.stopped().await;

    Ok(())
}
