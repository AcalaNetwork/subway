#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // read config from file
    let cli = subway::cli::parse_args();
    let config = subway::config::read_config(&cli.config)?;

    subway::logger::enable_logger();
    tracing::trace!("{:#?}", config);

    let subway_server = subway::server::build(config).await?;
    tracing::info!("Server running at {}", subway_server.addr);

    subway_server.handle.stopped().await;

    opentelemetry::global::shutdown_tracer_provider();

    Ok(())
}
