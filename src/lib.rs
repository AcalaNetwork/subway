pub mod api;
pub mod cache;
pub mod client;
pub mod config;
pub mod helper;
pub mod middleware;
pub mod server;
pub mod telemetry;

#[cfg(test)]
mod integration_tests;

use opentelemetry::sdk::trace::Tracer;
use tracing_subscriber::prelude::*;

pub fn enable_logger(tracer: Option<Tracer>) {
    let registry = tracing_subscriber::registry();

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::Level::INFO.into())
        .from_env_lossy();

    let log_format = std::env::var("LOG_FORMAT")
        .unwrap_or_default()
        .to_lowercase();

    #[cfg(tokio_unstable)]
    {
        let console_layer = console_subscriber::ConsoleLayer::builder()
            .with_default_env()
            .spawn();

        let fmt_layer = tracing_subscriber::fmt::layer();
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

        if tracer.is_some() {
            tracing::warn!("OpenTelemetry is not supported with the cfg(tokio_unstable)")
        }
    }

    // can't figure out how to get this code type checked without duplicated code
    // but it works with duplicated code so let it be

    #[cfg(not(tokio_unstable))]
    {
        if let Some(tracer) = tracer {
            let _ = match log_format.as_str() {
                "json" => registry
                    .with(tracing_opentelemetry::layer().with_tracer(tracer))
                    .with(tracing_subscriber::fmt::layer().json().with_filter(filter))
                    .try_init(),
                "pretty" => registry
                    .with(tracing_opentelemetry::layer().with_tracer(tracer))
                    .with(
                        tracing_subscriber::fmt::layer()
                            .pretty()
                            .with_filter(filter),
                    )
                    .try_init(),
                "compact" => registry
                    .with(tracing_opentelemetry::layer().with_tracer(tracer))
                    .with(
                        tracing_subscriber::fmt::layer()
                            .compact()
                            .with_filter(filter),
                    )
                    .try_init(),
                _ => registry
                    .with(tracing_opentelemetry::layer().with_tracer(tracer))
                    .with(tracing_subscriber::fmt::layer().with_filter(filter))
                    .try_init(),
            };
        } else {
            let fmt_layer = tracing_subscriber::fmt::layer();

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
    }
}
