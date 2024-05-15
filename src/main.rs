#[tokio::main]
async fn main() -> anyhow::Result<()> {
    subway::logger::enable_logger();
    let cli = subway::cli::parse_args();
    let config = subway::config::read_config(&cli.config)?;
    tracing::trace!("{:#?}", config);

    let validate_res = subway::config::validate(&config).await;
    if let Err(err) = &validate_res {
        tracing::error!("Config validation failed: {err:?}");
    }
    // early return if we're just validating the config
    if cli.is_validate() {
        return validate_res;
    }

    let subway_server = subway::server::build(config).await?;
    tracing::info!("Server running at {}", subway_server.addr);

    subway_server.handle.stopped().await;

    opentelemetry::global::shutdown_tracer_provider();

    Ok(())
}
