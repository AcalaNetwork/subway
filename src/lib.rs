pub mod api;
pub mod cache;
pub mod client;
pub mod config;
pub mod middleware;
pub mod server;

#[cfg(test)]
mod integration_tests;

use tracing_subscriber::prelude::*;

pub fn enable_logger() {
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

        let _ = match log_format.as_str() {
            "json" => log_layer
                .with(fmt_layer.json().with_filter(filter))
                .try_init(),
            "pretty" => log_layer
                .with(fmt_layer.pretty().with_filter(filter))
                .try_init(),
            "compact" => log_layer
                .with(fmt_layer.compact().with_filter(filter))
                .try_init(),
            _ => log_layer.with(fmt_layer.with_filter(filter)).try_init(),
        };
    }

    #[cfg(not(tokio_unstable))]
    let _ = match log_format.as_str() {
        "json" => registry
            .with(fmt_layer.json().with_filter(filter))
            .try_init(),
        "pretty" => registry
            .with(fmt_layer.pretty().with_filter(filter))
            .try_init(),
        "compact" => registry
            .with(fmt_layer.compact().with_filter(filter))
            .try_init(),
        _ => registry.with(fmt_layer.with_filter(filter)).try_init(),
    };
}
