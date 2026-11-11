// tests/sqlite/index_tests.rs
use crate::common::{get_latest_migration_content, simulate_tern_generate};
use std::fs;

pub async fn test_sqlite_index_entities() {
    let base_dir = "tests/temp_sqlite_index_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    let v1 = "
        CREATE TABLE users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL,
            email TEXT NOT NULL
        );
        CREATE INDEX idx_users_username ON users(username);
        CREATE UNIQUE INDEX idx_users_email ON users(email);
    ";
    simulate_tern_generate(
        "sqlite",
        "none",
        v1,
        &migrations_dir,
        &schema_path,
        "v1_indexes",
    )
    .await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE INDEX idx_users_username ON users (username);"));
    assert!(v1_sql.contains("CREATE UNIQUE INDEX idx_users_email ON users (email);"));

    // V2: Drop one index, add composite index
    let v2 = "
        CREATE TABLE users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL,
            email TEXT NOT NULL
        );
        CREATE INDEX idx_users_composite ON users(username, email);
    ";
    simulate_tern_generate(
        "sqlite",
        "none",
        v2,
        &migrations_dir,
        &schema_path,
        "v2_indexes",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(v2_sql.contains("DROP INDEX idx_users_username;"));
    assert!(v2_sql.contains("DROP INDEX idx_users_email;"));
    assert!(v2_sql.contains("CREATE INDEX idx_users_composite ON users (username, email);"));

    let _ = fs::remove_dir_all(base_dir);
}
