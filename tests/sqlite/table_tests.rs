// tests/sqlite/table_tests.rs
use crate::common::{get_latest_migration_content, simulate_tern_generate};
use std::fs;

pub async fn test_sqlite_table_entities() {
    let base_dir = "tests/temp_sqlite_table_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    // V1: Initial Tables based on tests/schemas/sqlite.sql
    let v1 = "
        CREATE TABLE users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE posts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT,
            title TEXT NOT NULL,
            body TEXT,
            FOREIGN KEY(user_id) REFERENCES users(id)
        );
    ";
    simulate_tern_generate(
        "sqlite",
        "none",
        v1,
        &migrations_dir,
        &schema_path,
        "v1_init",
    )
    .await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE TABLE users"));
    assert!(v1_sql.contains("CREATE TABLE posts"));

    // V2: Alter Columns (add email, drop body)
    let v2 = "
        CREATE TABLE users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            email TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE posts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT,
            title TEXT NOT NULL,
            FOREIGN KEY(user_id) REFERENCES users(id)
        );
    ";
    simulate_tern_generate(
        "sqlite",
        "none",
        v2,
        &migrations_dir,
        &schema_path,
        "v2_columns",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(v2_sql.contains("ALTER TABLE users ADD COLUMN email TEXT NOT NULL;"));
    assert!(v2_sql.contains("ALTER TABLE posts DROP COLUMN body;"));

    // V3: Drop Table
    let v3 = "
        CREATE TABLE users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            email TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
    ";
    simulate_tern_generate(
        "sqlite",
        "none",
        v3,
        &migrations_dir,
        &schema_path,
        "v3_drop",
    )
    .await;
    let v3_sql = get_latest_migration_content(&migrations_dir);
    assert!(v3_sql.contains("DROP TABLE posts;"));

    let _ = fs::remove_dir_all(base_dir);
}
