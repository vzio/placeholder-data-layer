use std::error::Error;

use server::config::Config;
use server::http::{AppState, router};
use server::telemetry;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = Config::from_env()?;
    telemetry::init(&config);

    let bind = config.bind;
    let app = router(AppState::new(config)?);
    let listener = tokio::net::TcpListener::bind(bind).await?;

    tracing::info!(%bind, "data layer listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install the shutdown handler");
    }
}
