mod api;
mod cache;
mod client;
mod config;
mod middleware;
mod server;

use tracing_subscriber::prelude::*;

fn enable_logger() {
    let registry = tracing_subscriber::registry();

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::Level::INFO.into())
        .from_env_lossy();

    let fmt_layer = tracing_subscriber::fmt::layer();

    let log_format = std::env::var("LOG_FORMAT")
        .unwrap_or_default()
        .to_lowercase();

    #[cfg(tokio_unstable)]
    {
        let console_layer = console_subscriber::ConsoleLayer::builder()
            .with_default_env()
            .spawn();

        let log_layer = registry.with(console_layer);

        match log_format.as_str() {
            "json" => log_layer.with(fmt_layer.json().with_filter(filter)).init(),
            "pretty" => log_layer
                .with(fmt_layer.pretty().with_filter(filter))
                .init(),
            "compact" => log_layer
                .with(fmt_layer.compact().with_filter(filter))
                .init(),
            _ => log_layer.with(fmt_layer.with_filter(filter)).init(),
        };
    }

    #[cfg(not(tokio_unstable))]
    match log_format.as_str() {
        "json" => registry.with(fmt_layer.json().with_filter(filter)).init(),
        "pretty" => registry.with(fmt_layer.pretty().with_filter(filter)).init(),
        "compact" => registry
            .with(fmt_layer.compact().with_filter(filter))
            .init(),
        _ => registry.with(fmt_layer.with_filter(filter)).init(),
    };
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    enable_logger();

    let config = match config::read_config() {
        Ok(config) => config,
        Err(e) => {
            return Err(anyhow::anyhow!(e));
        }
    };

    let client = client::create_client(&config).await?;

    let (addr, handle) = server::start_server(&config, client).await?;

    tracing::info!("Server running at {addr}");

    handle.await?;

    Ok(())
}
