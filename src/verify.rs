use crate::cli::resolve_target_url;
use crate::config::{self, TernConfig};
use crate::db;
use crate::differential_engine::parser::parse_target_schema;
use std::fs;
use std::path::Path;

/// Executes complete verification suitable for CI/CD environments.
/// Validates:
/// 1. tern.toml project configuration.
/// 2. Target declarative SQL schema syntax.
/// 3. Migrations lockfile existence and SHA-256 checksums (detects modifications, additions, and deletions).
/// 4. Remote database checksum alignment and drift (if database connection is configured).
pub async fn run_verify(
    cli_url: Option<String>,
    config_file: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("-> Running Tern verification suite (CI/CD validation mode)...\n");

    // 1. Verify Configuration File
    let config_path = config_file.unwrap_or_else(|| config::CONFIG_FILE.to_string());
    if !Path::new(&config_path).exists() {
        return Err(format!(
            "Configuration file '{}' not found. Run `tern init` to initialize the project.",
            config_path
        )
        .into());
    }

    let config_content = fs::read_to_string(&config_path)?;
    let config: TernConfig = toml::from_str(&config_content)
        .map_err(|e| format!("Failed to parse '{}': {}", config_path, e))?;

    let valid_dbs = ["postgres", "mysql", "sqlite"];
    if !valid_dbs.contains(&config.project.db_type.to_lowercase().as_str()) {
        return Err(format!(
            "Invalid database dialect '{}' in '{}'. Expected one of: postgres, mysql, sqlite.",
            config.project.db_type, config_path
        )
        .into());
    }

    println!(
        "✓ [1/4] Project configuration '{}' verified (Dialect: {}, Engine: {}).",
        config_path, config.project.db_type, config.project.engine
    );

    // 2. Verify Target Declarative Schema DDL Syntax
    let target_schema = match parse_target_schema(
        &config.project.db_type,
        &config.paths.schema_path,
        &config,
    ) {
        Ok(s) => {
            println!(
                "✓ [2/4] Target schema DDL syntax valid at '{}' (Found {} table(s), {} enum(s), {} function(s), {} trigger(s)).",
                config.paths.schema_path,
                s.tables.len(),
                s.enums.len(),
                s.functions.len(),
                s.triggers.len()
            );
            s
        }
        Err(e) => {
            return Err(format!(
                "Schema Validation Error in '{}': {}",
                config.paths.schema_path, e
            )
            .into());
        }
    };
    let _ = target_schema;

    // 3. Verify Migration Files & Lockfile Integrity (SHA-256 Checksums)
    config::verify_lockfile(&config.paths.migrations_dir)?;
    let lock = config::compute_migrations_lock(&config.paths.migrations_dir)?;
    if lock.files.is_empty() {
        println!(
            "✓ [3/4] Migrations directory '{}' is clean (0 migrations found).",
            config.paths.migrations_dir
        );
    } else {
        println!(
            "✓ [3/4] All {} migration checksum(s) verified against '{}' (Total Hash: {}...).",
            lock.files.len(),
            config::LOCK_FILE,
            &lock.total_hash[..12.min(lock.total_hash.len())]
        );
    }

    // 4. Remote Database Check (if URL provided or configured)
    let target_url_opt = resolve_target_url(&cli_url, &config).ok();

    if let Some(url) = target_url_opt {
        println!("-> Verifying remote target database checksums...");
        let pool = db::connect_target(&url).await?;
        db::verify_target_checksums(&pool, &config.paths.migrations_dir).await?;
        println!("✓ [4/4] Remote database verified with 0 drift.");
    } else {
        println!("✓ [4/4] Remote database check skipped (no database URL provided).");
    }

    println!("\n-> [SUCCESS] All verification checks passed! Schemas and checksums are intact.");
    Ok(())
}
