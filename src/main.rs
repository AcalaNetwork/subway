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
    let _ = match log_format {
        Ok(log_format) => match log_format.to_lowercase().as_str() {
            "json" => builder.json().try_init(),
            "pretty" => builder.pretty().try_init(),
            "compact" => builder.compact().try_init(),
            "full" | "" => builder.try_init(),
            _ => {
                let res = builder.try_init();
                tracing::warn!("Unknown log format: {log_format}");
                res
            }
        },
        Err(_) => builder.try_init(),
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
