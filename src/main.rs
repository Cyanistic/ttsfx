#[tokio::main]
async fn main() -> ttsfx::Result<()> {
    dotenvy::dotenv().ok();
    color_eyre::install().ok();
    ttsfx::init_tracing(&[])?;
    let cli = ttsfx::cli::Cli::parse_args();
    ttsfx::run(cli.listen).await
}

