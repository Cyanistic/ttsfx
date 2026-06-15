#[tokio::main]
async fn main() -> ttsfx::Result<()> {
    dotenvy::dotenv().ok();
    color_eyre::install().ok();
    tracing_subscriber::fmt::try_init().ok();
    ttsfx::run().await
}

