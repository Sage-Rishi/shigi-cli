// tests/sqlite_diff_test.rs
use std::fs;
use tern::config::{FilterConfig, PathConfig, ProjectConfig, TernConfig};
use tern::db;
use tern::diff;

#[tokio::test]
async fn test_sqlite_schema_diffing() {
    let migrations_dir = "tests/temp_sqlite_migrations";

    // Clean up any previous test runs
    let _ = fs::remove_dir_all(migrations_dir);
    fs::create_dir_all(migrations_dir).unwrap();

    // 1. Mock the configuration in memory (bypasses tern.toml)
    let config = TernConfig {
        project: ProjectConfig {
            db_type: "sqlite".to_string(),
            engine: "none".to_string(),
            target_url: None,
        },
        paths: PathConfig {
            schema_path: "tests/schemas/sqlite.sql".to_string(),
            migrations_dir: migrations_dir.to_string(),
        },
        filter: FilterConfig {
            include_tables: vec![],
            exclude_tables: vec!["_tern_migrations".to_string()],
        },
    };

    // 2. Connect to the ephemeral shadow database
    let (pool, db_url) = db::connect("sqlite")
        .await
        .expect("Failed to connect to SQLite shadow DB");

    // 3. Replay any existing migrations (will be 0 for the first run)
    db::apply_migrations(&pool, migrations_dir)
        .await
        .expect("Failed to apply migrations");

    // 4. Generate the diff based on tests/schemas/sqlite.sql
    diff::generate_diff(&config, &db_url, "test_sqlite_init")
        .await
        .expect("Failed to generate diff");

    // 5. Assertions
    let paths = fs::read_dir(migrations_dir).unwrap();
    let mut sql_files = Vec::new();
    let mut lock_file_exists = false;

    for path in paths {
        let file_name = path.unwrap().file_name().into_string().unwrap();
        if file_name.ends_with(".sql") {
            sql_files.push(file_name);
        } else if file_name == "tern.lock" {
            lock_file_exists = true;
        }
    }

    assert_eq!(
        sql_files.len(),
        1,
        "Expected exactly 1 migration file to be generated"
    );
    assert!(
        sql_files[0].contains("test_sqlite_init"),
        "Migration file missing name suffix"
    );
    assert!(lock_file_exists, "tern.lock file was not generated");

    // Cleanup shadow DB and temp folder
    let _ = fs::remove_file("tern_shadow.db");
    let _ = fs::remove_dir_all(migrations_dir);
}
