// tests/postgres/table_tests.rs
use crate::common::{
    detect_working_container_engine, get_latest_migration_content, simulate_tern_generate,
};
use std::fs;

pub async fn test_postgres_table_entities() {
    let engine = match detect_working_container_engine().await {
        Some(e) => e,
        None => {
            println!("Skipping Postgres table test: no accessible container engine running.");
            return;
        }
    };

    let base_dir = "tests/temp_pg_table_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    // V1: Based on tests/schemas/postgres.sql
    let v1 = "
        CREATE TABLE users (
            id UUID PRIMARY KEY,
            username VARCHAR(50) NOT NULL UNIQUE,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE posts (
            id SERIAL PRIMARY KEY,
            user_id UUID REFERENCES users(id),
            title VARCHAR(255) NOT NULL,
            body TEXT
        );
    ";
    simulate_tern_generate(
        "postgres",
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

    // V2: Alter Columns (add email, change title to TEXT, drop body)
    let v2 = "
        CREATE TABLE users (
            id UUID PRIMARY KEY,
            username VARCHAR(50) NOT NULL UNIQUE,
            email VARCHAR(255) NOT NULL,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE posts (
            id SERIAL PRIMARY KEY,
            user_id UUID REFERENCES users(id),
            title TEXT NOT NULL
        );
    ";
    simulate_tern_generate(
        "postgres",
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
        v2_sql.contains("ALTER COLUMN title TYPE text")
            || v2_sql.contains("ALTER COLUMN title TYPE TEXT")
    );
    assert!(v2_sql.contains("DROP COLUMN body"));

    // V3: Drop Table
    let v3 = "
        CREATE TABLE users (
            id UUID PRIMARY KEY,
            username VARCHAR(50) NOT NULL UNIQUE,
            email VARCHAR(255) NOT NULL,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
        );
    ";
    simulate_tern_generate(
        "postgres",
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
