#[tokio::main]
async fn main() -> ttsfx::Result<()> {
    color_eyre::install().ok();
    tracing_subscriber::fmt::try_init().ok();
    ttsfx::run().await
}
