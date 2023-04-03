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

    let client = client::create_client(&config).await?;

    let (addr, server) = server::start_server(&config, client).await?;

    tracing::info!("Server running at {addr}");

    server.stopped().await;

    Ok(())
}
