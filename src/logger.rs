use chrono::Utc;
use opentelemetry::trace::TraceContextExt;
use opentelemetry::Context;
use serde::ser::{SerializeMap, Serializer as _};
use std::io;
use tracing::{Event, Subscriber};
use tracing_serde::fields::AsMap;
use tracing_serde::AsSerde;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;

pub struct TraceIdFormat;

impl<S, N> FormatEvent<S, N> for TraceIdFormat
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(&self, _ctx: &FmtContext<'_, S, N>, mut writer: Writer<'_>, event: &Event<'_>) -> std::fmt::Result
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let meta = event.metadata();

        let mut visit = || {
            let mut serializer = serde_json::Serializer::new(WriteAdaptor::new(&mut writer));
            let mut serializer = serializer.serialize_map(None)?;
            serializer.serialize_entry("timestamp", &Utc::now().to_rfc3339())?;
            serializer.serialize_entry("level", &meta.level().as_serde())?;
            serializer.serialize_entry("fields", &event.field_map())?;
            serializer.serialize_entry("target", meta.target())?;

            let cx = Context::current();
            if cx.has_active_span() {
                let span = cx.span();
                let span_cx = span.span_context();

                let trace_id = u128::from_be_bytes(span_cx.trace_id().to_bytes());
                let trace_id = trace_id as u64; // take LSB
                let span_id = u64::from_be_bytes(span_cx.span_id().to_bytes());

                serializer.serialize_entry("trace_id", &trace_id)?;
                serializer.serialize_entry("span_id", &span_id)?;
            }

            serializer.end()
        };

        visit().map_err(|_| std::fmt::Error)?;
        writeln!(writer)
    }
}

pub struct WriteAdaptor<'a> {
    fmt_write: &'a mut dyn std::fmt::Write,
}

impl<'a> WriteAdaptor<'a> {
    pub fn new(fmt_write: &'a mut dyn std::fmt::Write) -> Self {
        Self { fmt_write }
    }
}

impl<'a> io::Write for WriteAdaptor<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = std::str::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        self.fmt_write
            .write_str(s)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        Ok(s.as_bytes().len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn enable_logger() {
    let registry = tracing_subscriber::registry();

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::Level::INFO.into())
        .from_env_lossy();

    let log_format = std::env::var("LOG_FORMAT").unwrap_or_default().to_lowercase();

    let fmt_layer = tracing_subscriber::fmt::layer();

    #[cfg(tokio_unstable)]
    let log_layer = {
        let console_layer = console_subscriber::ConsoleLayer::builder().with_default_env().spawn();

        registry.with(console_layer)
    };

    #[cfg(not(tokio_unstable))]
    let log_layer = registry;

    let _ = match log_format.as_str() {
        "json" => log_layer
            .with(fmt_layer.event_format(TraceIdFormat).with_filter(filter))
            .try_init(),
        "pretty" => log_layer.with(fmt_layer.pretty().with_filter(filter)).try_init(),
        "compact" => log_layer.with(fmt_layer.compact().with_filter(filter)).try_init(),
        _ => log_layer.with(fmt_layer.with_filter(filter)).try_init(),
    };
}
