use crate::config::compute_migrations_lock;
use chrono::Utc;
use sqlx::any::AnyPoolOptions;
use sqlx::{Any, Pool, Row};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::sleep;

pub type AnyPool = Pool<Any>;

pub async fn connect(db_type: &str) -> Result<(AnyPool, String), Box<dyn std::error::Error>> {
    sqlx::any::install_default_drivers();

    let url = match db_type.to_lowercase().as_str() {
        "mysql" => "mysql://root:tern_password@127.0.0.1:3306/tern_db".to_string(),
        "sqlite" => {
            let db_path = "tern_shadow.db";
            if Path::new(db_path).exists() {
                fs::remove_file(db_path)?;
            }
            File::create(db_path)?;
            format!("sqlite://{}", db_path)
        }
        _ => "postgres://postgres:tern_password@127.0.0.1:5432/postgres".to_string(),
    };

    println!(
        "-> Waiting for {} shadow database to accept connections...",
        db_type
    );

    let pool_options = AnyPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(3));

    let mut retries = 30;
    let pool = loop {
        match pool_options.clone().connect(&url).await {
            Ok(pool) => break pool,
            Err(e) => {
                if retries == 0 {
                    return Err(format!("Failed to connect after 30s: {}", e).into());
                }
                retries -= 1;
                sleep(Duration::from_secs(1)).await;
            }
        }
    };

    Ok((pool, url))
}

/// Connects directly to a user's target database (dev/staging/prod)
pub async fn connect_target(target_url: &str) -> Result<AnyPool, Box<dyn std::error::Error>> {
    sqlx::any::install_default_drivers();

    // Auto-create the SQLite database file if it doesn't exist yet
    if let Some(sqlite_path) = target_url.strip_prefix("sqlite://") {
        let clean_path = sqlite_path.split('?').next().unwrap_or(sqlite_path);
        if !clean_path.is_empty() && clean_path != ":memory:" && !Path::new(clean_path).exists() {
            if let Some(parent) = Path::new(clean_path).parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent)?;
                }
            }
            File::create(clean_path)?;
            println!("-> Created target SQLite database file: {}", clean_path);
        }
    }

    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(10))
        .connect(target_url)
        .await
        .map_err(|e| {
            format!(
                "Failed to connect to target database ({}): {}",
                target_url, e
            )
        })?;

    Ok(pool)
}

/// Ensures the `_tern_migrations` state tracking table exists on the target database
async fn ensure_migrations_table(pool: &AnyPool) -> Result<(), Box<dyn std::error::Error>> {
    let create_table_sql = "
        CREATE TABLE IF NOT EXISTS _tern_migrations (
            filename VARCHAR(255) PRIMARY KEY,
            checksum VARCHAR(64) NOT NULL,
            applied_at VARCHAR(64) NOT NULL
        );
    ";
    sqlx::query(create_table_sql).execute(pool).await?;
    Ok(())
}

/// Fetches all previously applied migrations from `_tern_migrations`
async fn get_applied_migrations(
    pool: &AnyPool,
) -> Result<BTreeMap<String, (String, String)>, Box<dyn std::error::Error>> {
    ensure_migrations_table(pool).await?;

    let rows = sqlx::query(
        "SELECT filename, checksum, applied_at FROM _tern_migrations ORDER BY filename ASC",
    )
    .fetch_all(pool)
    .await?;

    let mut applied = BTreeMap::new();
    for row in rows {
        let filename: String = row.get("filename");
        let checksum: String = row.get("checksum");
        let applied_at: String = row.get("applied_at");
        applied.insert(filename, (checksum, applied_at));
    }

    Ok(applied)
}

/// Displays the status of local migrations against the target database
pub async fn check_status(
    pool: &AnyPool,
    migrations_dir: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let applied = get_applied_migrations(pool).await?;
    let local_lock = compute_migrations_lock(migrations_dir)?;

    if local_lock.files.is_empty() && applied.is_empty() {
        println!("-> No migrations found locally or in the target database.");
        return Ok(());
    }

    println!(
        "\n{:<38} {:<14} {:<26}",
        "MIGRATION FILE", "STATUS", "APPLIED AT"
    );
    println!("{}", "-".repeat(80));

    let mut pending_count = 0;
    let mut drift_detected = false;

    for (filename, local_hash) in &local_lock.files {
        match applied.get(filename) {
            Some((db_hash, applied_at)) => {
                if db_hash == local_hash {
                    println!("{:<38} {:<14} {:<26}", filename, "Applied", applied_at);
                } else {
                    println!(
                        "{:<38} {:<14} {:<26}",
                        filename, "DRIFT (Hash!)", applied_at
                    );
                    drift_detected = true;
                }
            }
            None => {
                println!("{:<38} {:<14} {:<26}", filename, "Pending", "-");
                pending_count += 1;
            }
        }
    }

    // Check for migrations recorded in the DB that are missing from the local disk
    for (db_file, (_, applied_at)) in &applied {
        if !local_lock.files.contains_key(db_file) {
            println!("{:<38} {:<14} {:<26}", db_file, "MISSING LOCAL", applied_at);
            drift_detected = true;
        }
    }

    println!("{}", "-".repeat(80));
    println!(
        "Summary: {} applied, {} pending.\n",
        applied.len(),
        pending_count
    );

    if drift_detected {
        return Err(
            "Checksum drift or missing local migration files detected! Resolve before applying."
                .into(),
        );
    }

    Ok(())
}

/// Applies pending migrations to the target database and records them in `_tern_migrations`
pub async fn apply_target_migrations(
    pool: &AnyPool,
    db_type: &str,
    migrations_dir: &str,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let applied = get_applied_migrations(pool).await?;
    let local_lock = compute_migrations_lock(migrations_dir)?;

    // 1. Verify integrity against already-applied database records
    for (filename, (db_hash, _)) in &applied {
        match local_lock.files.get(filename) {
            Some(local_hash) if local_hash != db_hash => {
                return Err(format!(
                    "Integrity Error: Applied migration '{}' was modified locally! DB checksum: {}, Local checksum: {}",
                    filename, db_hash, local_hash
                )
                .into());
            }
            None => {
                return Err(format!(
                    "Integrity Error: Migration '{}' is applied in the database but missing from '{}'!",
                    filename, migrations_dir
                )
                .into());
            }
            _ => {}
        }
    }

    // 2. Collect pending migrations in chronological order
    let mut pending: Vec<(String, String)> = Vec::new();
    for (filename, local_hash) in &local_lock.files {
        if !applied.contains_key(filename) {
            pending.push((filename.clone(), local_hash.clone()));
        }
    }

    if pending.is_empty() {
        println!("-> Target database is up to date! No pending migrations to apply.");
        return Ok(());
    }

    if dry_run {
        println!(
            "-> [Dry Run] {} pending migration(s) would be applied:",
            pending.len()
        );
        for (filename, _) in &pending {
            println!("   - {}", filename);
        }
        return Ok(());
    }

    println!(
        "-> Applying {} pending migration(s) to target database...",
        pending.len()
    );

    for (filename, checksum) in pending {
        let path = Path::new(migrations_dir).join(&filename);
        let sql = fs::read_to_string(&path)?;

        let mut tx = pool.begin().await?;

        for statement in sql.split(';') {
            let trimmed = statement.trim();
            if !trimmed.is_empty() {
                sqlx::query(trimmed).execute(&mut *tx).await.map_err(|e| {
                    format!(
                        "Failed executing '{}' in statement [{}]: {}",
                        filename, trimmed, e
                    )
                })?;
            }
        }

        let now = Utc::now().to_rfc3339();
        let insert_sql = match db_type.to_lowercase().as_str() {
            "postgres" => {
                "INSERT INTO _tern_migrations (filename, checksum, applied_at) VALUES ($1, $2, $3)"
            }
            _ => "INSERT INTO _tern_migrations (filename, checksum, applied_at) VALUES (?, ?, ?)",
        };

        sqlx::query(insert_sql)
            .bind(&filename)
            .bind(&checksum)
            .bind(&now)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        println!("   [Applied] {}", filename);
    }

    println!("-> Successfully applied all pending migrations!");
    Ok(())
}

/// Replays migrations on the ephemeral shadow database during `tern generate`
pub async fn apply_migrations(
    pool: &AnyPool,
    migrations_dir: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new(migrations_dir);
    if !dir.exists() {
        return Ok(());
    }

    let mut entries: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    entries.sort();

    if entries.is_empty() {
        println!("-> No existing migrations found. Comparing against empty DB.");
        return Ok(());
    }

    println!(
        "-> Replaying {} existing migration(s) on shadow DB...",
        entries.len()
    );
    for path in entries {
        let filename = path.file_name().unwrap().to_string_lossy();
        let sql = fs::read_to_string(&path)?;

        for statement in sql.split(';') {
            let trimmed = statement.trim();
            if !trimmed.is_empty() {
                sqlx::query(trimmed)
                    .execute(pool)
                    .await
                    .map_err(|e| format!("Error executing migration '{}': {}", filename, e))?;
            }
        }
        println!("   [Applied] {}", filename);
    }

    Ok(())
}
