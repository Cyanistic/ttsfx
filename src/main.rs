use clap::Parser;
use ttsfx::cli::{Cli, Commands};
use ttsfx::error::ResultExt;
use ttsfx::reindex::reindex_embeddings;

#[tokio::main]
async fn main() -> ttsfx::Result<()> {
    dotenvy::dotenv().ok();
    color_eyre::install().warn().ok();
    ttsfx::init_tracing(&[])?;
    let cli = Cli::parse();

    match cli.command.unwrap_or(Commands::Run) {
        Commands::Run => ttsfx::run(cli.listen, &cli.config).await,
        Commands::ReindexEmbeddings(args) => {
            let config = ttsfx::config::Config::load_from_path(&cli.config)?;
            reindex_embeddings(&config, &args).await
        }
    }
}