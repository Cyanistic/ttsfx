#[tokio::main]
async fn main() -> ttsfx::Result<()> {
    dotenvy::dotenv().ok();
    color_eyre::install().ok();
    ttsfx::init_tracing(&[])?;
    ttsfx::run().await
}

