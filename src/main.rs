mod cli;
mod db;
mod diff;
mod engine;

use clap::Parser;
use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let engine = match cli.engine {
        Some(eng) => eng.to_lowercase(),
        None => cli::prompt_for_engine().to_lowercase(),
    };

    match &cli.command {
        Some(Commands::Generate { db_type }) => {
            println!("Using container engine: {}", engine);
            engine::provision_container(&engine, db_type).await?;
            db::connect(db_type).await?;
            diff::generate_diff(db_type).await?;
            engine::teardown_container(&engine).await?;
        }
        None => {
            println!("No command provided. Try 'tern generate' or 'tern help'.");
        }
    }
    Ok(())
}
