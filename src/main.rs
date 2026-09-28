mod config;
mod engines;
mod fetchers;
mod tools;
mod transport;
mod types;
mod utils;

use anyhow::Result;
use tracing_subscriber::{EnvFilter, fmt};

use config::CONFIG;
use transport::Mode;

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mode = Mode::from_env()?;

    tracing::info!("ferris-search starting...");
    tracing::info!("Default engine: {}", CONFIG.default_search_engine);
    tracing::info!("Mode: {:?}", mode);

    transport::serve(mode).await
}
