#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // read config from file
    let config = match subway::config::read_config() {
        Ok(config) => config,
        Err(e) => {
            return Err(anyhow::anyhow!(e));
        }
    };

    subway::logger::enable_logger();
    tracing::trace!("{:#?}", config);

    let subway_server = subway::server::build(config).await?;
    tracing::info!("Server running at {}", subway_server.addr);

    subway_server.handle.stopped().await;

    opentelemetry::global::shutdown_tracer_provider();

    Ok(())
}
