use anyhow::Result;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("gadgetron=info".parse()?))
        .init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config/nexus.toml".to_string());

    let config = gadgetron_core::config::AppConfig::load(&config_path)
        .map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))?;

    tracing::info!("Gadgetron orchestrator starting on {}", config.server.bind);

    // TODO: Initialize providers, router, scheduler, node agents
    // TODO: Start gateway server
    // TODO: Start TUI if enabled

    Ok(())
}
