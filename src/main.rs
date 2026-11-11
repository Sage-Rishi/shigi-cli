// src/main.rs
use clap::Parser;
use tern::cli::{self, Cli, Commands};
use tern::config::{self, TernConfig};
use tern::db;
use tern::diff;
use tern::engine;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Init) => {
            cli::run_init_wizard()?;
        }
        Some(Commands::Generate { name }) => {
            let config = TernConfig::load()?;
            let engine = cli
                .engine
                .clone()
                .unwrap_or_else(|| config.project.engine.clone());
            let db_type = &config.project.db_type;

            config::verify_lockfile(&config.paths.migrations_dir)?;
            engine::provision_container(&engine, db_type).await?;

            let (pool, db_url) = db::connect(db_type).await?;
            db::apply_migrations(&pool, &config.paths.migrations_dir).await?;

            let diff_res = diff::generate_diff(&config, &db_url, name).await;

            if db_type == "sqlite" {
                let path = db_url.strip_prefix("sqlite://").unwrap_or("tern_shadow.db");
                let _ = std::fs::remove_file(path);
            } else {
                engine::teardown_container(&engine).await?;
            }

            diff_res?;
        }
        Some(Commands::Status { url }) => {
            let config = TernConfig::load()?;
            config::verify_lockfile(&config.paths.migrations_dir)?;

            let target_url = cli::resolve_target_url(url, &config)?;
            let pool = db::connect_target(&target_url).await?;

            db::check_status(&pool, &config.paths.migrations_dir).await?;
        }
        Some(Commands::Apply { url, dry_run }) => {
            let config = TernConfig::load()?;
            config::verify_lockfile(&config.paths.migrations_dir)?;

            let target_url = cli::resolve_target_url(url, &config)?;
            let pool = db::connect_target(&target_url).await?;

            db::apply_target_migrations(
                &pool,
                &config.project.db_type,
                &config.paths.migrations_dir,
                *dry_run,
            )
            .await?;
        }
        None => {
            println!("No command provided. Run `tern init`, `tern generate`, `tern status`, or `tern apply`.");
        }
    }

    Ok(())
}
