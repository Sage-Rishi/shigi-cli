// tests/mysql/table_tests.rs
use crate::common::{
    detect_working_container_engine, get_latest_migration_content, simulate_tern_generate,
};
use std::fs;

pub async fn test_mysql_table_entities() {
    let engine = match detect_working_container_engine().await {
        Some(e) => e,
        None => {
            println!("Skipping MySQL table test: no accessible container engine running.");
            return;
        }
    };

    let base_dir = "tests/temp_mysql_table_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    // V1: Based on tests/schemas/mysql.sql
    let v1 = "
        CREATE TABLE users (
            id VARCHAR(36) PRIMARY KEY,
            username VARCHAR(50) NOT NULL UNIQUE,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE posts (
            id INT AUTO_INCREMENT PRIMARY KEY,
            user_id VARCHAR(36),
            title VARCHAR(255) NOT NULL,
            body TEXT,
            FOREIGN KEY (user_id) REFERENCES users(id)
        );
    ";
    simulate_tern_generate(
        "mysql",
        &engine,
        v1,
        &migrations_dir,
        &schema_path,
        "v1_init",
    )
    .await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE TABLE users"));
    assert!(v1_sql.contains("CREATE TABLE posts"));

    // V2: Alter Columns (add email, modify title to TEXT, drop body)
    let v2 = "
        CREATE TABLE users (
            id VARCHAR(36) PRIMARY KEY,
            username VARCHAR(50) NOT NULL UNIQUE,
            email VARCHAR(255) NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE posts (
            id INT AUTO_INCREMENT PRIMARY KEY,
            user_id VARCHAR(36),
            title TEXT NOT NULL,
            FOREIGN KEY (user_id) REFERENCES users(id)
        );
    ";
    simulate_tern_generate(
        "mysql",
        &engine,
        v2,
        &migrations_dir,
        &schema_path,
        "v2_alter",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(v2_sql.contains("ADD COLUMN email"));
    assert!(
        v2_sql.contains("MODIFY COLUMN title text") || v2_sql.contains("MODIFY COLUMN title TEXT")
    );
    assert!(v2_sql.contains("DROP COLUMN body"));

    // V3: Drop Table
    let v3 = "
        CREATE TABLE users (
            id VARCHAR(36) PRIMARY KEY,
            username VARCHAR(50) NOT NULL UNIQUE,
            email VARCHAR(255) NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );
    ";
    simulate_tern_generate(
        "mysql",
        &engine,
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
