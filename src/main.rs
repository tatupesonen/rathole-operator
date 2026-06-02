use anyhow::Result;
use tracing_subscriber::{EnvFilter, FmtSubscriber};

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,rathole_operator=info"));
    FmtSubscriber::builder().with_env_filter(filter).init();

    let client = kube::Client::try_default().await?;
    tracing::info!("starting rathole-operator");
    rathole_operator::controller::run(client).await;
    Ok(())
}
