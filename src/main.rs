use crate::{config::read_config, server::start_server};

mod config;
mod server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .try_init()
        .expect("setting default subscriber failed");

    let config = match read_config() {
        Ok(config) => config,
        Err(e) => {
            return Err(anyhow::anyhow!(e));
        }
    };

    let (addr, handle) = start_server(config)
    .await?;

    tracing::info!("Server running at {}", addr);

    handle.await?;

    Ok(())
}
