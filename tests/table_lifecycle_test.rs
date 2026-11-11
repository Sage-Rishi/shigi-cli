use std::fs;
use std::path::{Path, PathBuf};
use tern::config::{FilterConfig, PathConfig, ProjectConfig, TernConfig};
use tern::db;
use tern::diff;
use tern::engine;

/// Helper to safely find an available container engine without interactive prompts
fn detect_headless_engine() -> Option<String> {
    if Path::new("/var/run/docker.sock").exists()
        || Path::new("/Users/bitsage/.docker/run/docker.sock").exists()
    {
        return Some("docker".to_string());
    }
    if Path::new(
        "/Users/bitsage/.local/share/containers/podman/machine/podman-machine-default/api.sock",
    )
    .exists()
    {
        return Some("podman".to_string());
    }
    None
}

/// Helper to read the most recently generated migration file in a directory
fn get_latest_migration_content(dir: &str) -> String {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    entries.sort(); // Chronological sort by filename prefix

    let latest = entries.last().expect("No migration file found");
    fs::read_to_string(latest).unwrap()
}

/// Core simulation of the `tern generate` command lifecycle
async fn simulate_tern_generate(
    db_type: &str,
    engine: &str,
    schema_sql: &str,
    migrations_dir: &str,
    schema_path: &str,
    step_name: &str,
) {
    // Write the desired schema state
    fs::write(schema_path, schema_sql).unwrap();

    let config = TernConfig {
        project: ProjectConfig {
            db_type: db_type.to_string(),
            engine: engine.to_string(),
            target_url: None,
        },
        paths: PathConfig {
            schema_path: schema_path.to_string(),
            migrations_dir: migrations_dir.to_string(),
        },
        filter: FilterConfig {
            include_tables: vec![],
            exclude_tables: vec!["_tern_migrations".to_string()],
        },
    };

    // Phase 1: Provision
    engine::provision_container(engine, db_type).await.unwrap();

    // Phase 2: Connect & Replay
    let (pool, db_url) = db::connect(db_type).await.unwrap();
    db::apply_migrations(&pool, migrations_dir).await.unwrap();

    // Phase 3: Diff
    diff::generate_diff(&config, &db_url, step_name)
        .await
        .unwrap();

    // Phase 4: Teardown
    if db_type == "sqlite" {
        let _ = fs::remove_file("tern_shadow.db");
    } else {
        engine::teardown_container(engine).await.unwrap();
    }
}

async fn run_table_lifecycle(
    db_type: &str,
    engine: &str,
    v1_sql: &str,
    v2_sql: &str,
    v3_sql: &str,
    expected_v2_keywords: &[&str],
) {
    println!(
        "\n=== Starting Lifecycle Test for {} ===",
        db_type.to_uppercase()
    );

    let base_dir = format!("tests/temp_{}", db_type);
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    // Setup fresh directories
    let _ = fs::remove_dir_all(&base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    // --- V1: Table Creation ---
    simulate_tern_generate(
        db_type,
        engine,
        v1_sql,
        &migrations_dir,
        &schema_path,
        "v1_create",
    )
    .await;
    let v1_content = get_latest_migration_content(&migrations_dir);
    assert!(v1_content.contains("CREATE TABLE users"));
    assert!(v1_content.contains("CREATE TABLE posts"));

    // --- V2: Table Mutation (Add, Modify, Drop Columns) ---
    simulate_tern_generate(
        db_type,
        engine,
        v2_sql,
        &migrations_dir,
        &schema_path,
        "v2_alter",
    )
    .await;
    let v2_content = get_latest_migration_content(&migrations_dir);
    for keyword in expected_v2_keywords {
        assert!(
            v2_content.contains(keyword),
            "{} V2 diff missing expected keyword: '{}'. File content:\n{}",
            db_type,
            keyword,
            v2_content
        );
    }

    // --- V3: Table Destruction ---
    simulate_tern_generate(
        db_type,
        engine,
        v3_sql,
        &migrations_dir,
        &schema_path,
        "v3_drop",
    )
    .await;
    let v3_content = get_latest_migration_content(&migrations_dir);
    assert!(v3_content.contains("DROP TABLE posts;"));

    // Cleanup
    let _ = fs::remove_dir_all(&base_dir);
    println!(
        "=== Passed Lifecycle Test for {} ===\n",
        db_type.to_uppercase()
    );
}

#[tokio::test]
async fn test_all_db_table_lifecycles() {
    // 1. SQLite Tests
    let sqlite_v1 = "
        CREATE TABLE users (id TEXT PRIMARY KEY, name TEXT);
        CREATE TABLE posts (id INTEGER PRIMARY KEY AUTOINCREMENT);
    ";
    let sqlite_v2 = "
        CREATE TABLE users (id TEXT PRIMARY KEY, email TEXT NOT NULL);
        CREATE TABLE posts (id INTEGER PRIMARY KEY AUTOINCREMENT);
    ";
    let sqlite_v3 = "
        CREATE TABLE users (id TEXT PRIMARY KEY, email TEXT NOT NULL);
    ";

    // SQLite supports DROP COLUMN natively as of 3.35+
    run_table_lifecycle(
        "sqlite",
        "none",
        sqlite_v1,
        sqlite_v2,
        sqlite_v3,
        &["ADD COLUMN email", "DROP COLUMN name"],
    )
    .await;

    // Detect if we can run Postgres/MySQL tests
    let container_engine = match detect_headless_engine() {
        Some(engine) => engine,
        None => {
            println!("No container engine (Docker/Podman) found. Skipping Postgres and MySQL lifecycle tests.");
            return;
        }
    };

    // 2. Postgres Tests
    let pg_v1 = "
        CREATE TABLE users (id UUID PRIMARY KEY, name VARCHAR(50));
        CREATE TABLE posts (id SERIAL PRIMARY KEY);
    ";
    let pg_v2 = "
        CREATE TABLE users (id UUID PRIMARY KEY, email VARCHAR(100), name TEXT);
        CREATE TABLE posts (id SERIAL PRIMARY KEY);
    ";
    let pg_v3 = "
        CREATE TABLE users (id UUID PRIMARY KEY, email VARCHAR(100), name TEXT);
    ";

    run_table_lifecycle(
        "postgres",
        &container_engine,
        pg_v1,
        pg_v2,
        pg_v3,
        &["ADD COLUMN email", "ALTER COLUMN name TYPE TEXT"],
    )
    .await;

    // 3. MySQL Tests
    let mysql_v1 = "
        CREATE TABLE users (id VARCHAR(36) PRIMARY KEY, name VARCHAR(50));
        CREATE TABLE posts (id INT AUTO_INCREMENT PRIMARY KEY);
    ";
    let mysql_v2 = "
        CREATE TABLE users (id VARCHAR(36) PRIMARY KEY, email VARCHAR(100), name TEXT);
        CREATE TABLE posts (id INT AUTO_INCREMENT PRIMARY KEY);
    ";
    let mysql_v3 = "
        CREATE TABLE users (id VARCHAR(36) PRIMARY KEY, email VARCHAR(100), name TEXT);
    ";

    run_table_lifecycle(
        "mysql",
        &container_engine,
        mysql_v1,
        mysql_v2,
        mysql_v3,
        &["ADD COLUMN email", "MODIFY COLUMN name TEXT"],
    )
    .await;
}
