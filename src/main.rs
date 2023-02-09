mod api;
mod cache;
mod client;
mod config;
mod middleware;
mod server;

fn enable_logger() {
    let log_format = std::env::var("LOG_FORMAT");
    let builder = tracing_subscriber::FmtSubscriber::builder().with_env_filter(
        tracing_subscriber::EnvFilter::builder()
            .with_default_directive(tracing::Level::INFO.into())
            .from_env_lossy(),
    );
    if log_format.is_ok() && log_format.unwrap().to_lowercase() == "json" {
        let _ = builder.json().try_init();
    } else {
        let _ = builder.try_init();
    }
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
