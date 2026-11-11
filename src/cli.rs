use crate::config::{FilterConfig, PathConfig, ProjectConfig, TernConfig};
use clap::{Parser, Subcommand};
use inquire::{Select, Text};
use std::fs;
use std::path::Path;

#[derive(Parser)]
#[command(
    name = "tern",
    about = "Declarative SQL schema diff & migration generator"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Override container engine from tern.toml
    #[arg(short, long, global = true)]
    pub engine: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize tern.toml, schema directory, and migrations directory
    Init,
    /// Replay migrations, diff against target schema, and output a new migration script
    Generate {
        /// Optional name suffix for the generated migration file
        #[arg(short, long, default_value = "auto_diff")]
        name: String,
    },
    /// Verify migration integrity, checksums, lockfile, and schema validity (suitable for CI/CD)
    Verify {
        /// Target database connection URL (overrides tern.toml and DATABASE_URL)
        #[arg(short, long)]
        url: Option<String>,

        /// Path to configuration file (defaults to tern.toml)
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Check which migrations have been applied to the target database
    Status {
        /// Target database connection URL (overrides tern.toml and DATABASE_URL)
        #[arg(short, long)]
        url: Option<String>,
    },
    /// Apply pending migrations to the target database
    Apply {
        /// Target database connection URL (overrides tern.toml and DATABASE_URL)
        #[arg(short, long)]
        url: Option<String>,

        /// Preview pending migrations without executing them
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

pub fn resolve_target_url(
    cli_url: &Option<String>,
    config: &TernConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(u) = cli_url {
        return Ok(u.clone());
    }
    if let Ok(env_url) = std::env::var("DATABASE_URL") {
        if !env_url.trim().is_empty() {
            return Ok(env_url);
        }
    }
    if let Some(cfg_url) = &config.project.target_url {
        if !cfg_url.trim().is_empty() {
            return Ok(cfg_url.clone());
        }
    }
    Err("No target database URL provided. Pass `--url <URL>`, set `DATABASE_URL`, or configure `target_url` in tern.toml.".into())
}

pub fn run_init_wizard() -> Result<(), Box<dyn std::error::Error>> {
    println!("Initializing Tern project configuration...");

    let db_type = Select::new(
        "Which database engine are you using?",
        vec!["postgres", "mysql", "sqlite"],
    )
    .prompt()?
    .to_string();

    let engine = if db_type == "sqlite" {
        "none".to_string()
    } else {
        prompt_for_engine().to_lowercase()
    };

    let default_url = match db_type.as_str() {
        "postgres" => "postgres://postgres:password@localhost:5432/mydb",
        "mysql" => "mysql://root:password@localhost:3306/mydb",
        _ => "sqlite://dev.db",
    };

    let target_url_input = Text::new(
        "Target database URL for `status`/`apply` (leave blank to use DATABASE_URL env):",
    )
    .with_default(default_url)
    .prompt()?;

    let target_url = if target_url_input.trim().is_empty() {
        None
    } else {
        Some(target_url_input.trim().to_string())
    };

    let schema_path = Text::new("Where are your target schema files located (file or folder)?")
        .with_default("schema")
        .prompt()?;

    let migrations_dir = Text::new("Where should generated migrations be stored?")
        .with_default("migrations")
        .prompt()?;

    let exclude_raw = Text::new("Tables to exclude from diffing (comma-separated):")
        .with_default("_tern_migrations, _sqlx_migrations")
        .prompt()?;

    let include_raw =
        Text::new("Tables to exclusively include (comma-separated, leave blank for all):")
            .with_default("")
            .prompt()?;

    let mut exclude_tables: Vec<String> = exclude_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if !exclude_tables.iter().any(|t| t == "_tern_migrations") {
        exclude_tables.push("_tern_migrations".to_string());
    }

    let include_tables = include_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let exclude_schemas_raw =
        Text::new("Schemas to exclude from diffing (comma-separated, leave blank for none):")
            .with_default("")
            .prompt()?;

    let include_schemas_raw =
        Text::new("Schemas to exclusively include (comma-separated, leave blank for all):")
            .with_default("")
            .prompt()?;

    let exclude_schemas = exclude_schemas_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let include_schemas = include_schemas_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let config = TernConfig {
        project: ProjectConfig {
            db_type,
            engine,
            target_url,
        },
        paths: PathConfig {
            schema_path: schema_path.clone(),
            migrations_dir: migrations_dir.clone(),
        },
        filter: FilterConfig {
            include_tables,
            exclude_tables,
            include_schemas,
            exclude_schemas,
        },
    };

    config.save()?;
    fs::create_dir_all(&migrations_dir)?;

    if !schema_path.ends_with(".sql") {
        fs::create_dir_all(&schema_path)?;
    }

    println!("-> Created tern.toml and initialized directories.");
    Ok(())
}

pub fn prompt_for_engine() -> String {
    let mut available_engines = Vec::new();

    if Path::new("/var/run/docker.sock").exists()
        || Path::new("/Users/bitsage/.docker/run/docker.sock").exists()
    {
        available_engines.push("Docker");
    }

    if Path::new(
        "/Users/bitsage/.local/share/containers/podman/machine/podman-machine-default/api.sock",
    )
    .exists()
    {
        available_engines.push("Podman");
    }

    if available_engines.is_empty() {
        eprintln!("No running container daemons found. Defaulting to Docker in config.");
        return "docker".to_string();
    }

    if available_engines.len() == 1 {
        return available_engines[0].to_string();
    }

    Select::new(
        "Select container engine for shadow database:",
        available_engines,
    )
    .prompt()
    .unwrap_or("Docker")
    .to_string()
}
