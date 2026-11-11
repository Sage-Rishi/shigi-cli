// tests/postgres/enum_tests.rs
use crate::common::{
    detect_working_container_engine, get_latest_migration_content, simulate_tern_generate,
};
use std::fs;

pub async fn test_postgres_enum_entities() {
    let engine = match detect_working_container_engine().await {
        Some(e) => e,
        None => {
            println!("Skipping Postgres enum test: no accessible container engine running.");
            return;
        }
    };

    let base_dir = "tests/temp_pg_enum_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    // V1: Create ENUM and table using it
    let v1 = "
        CREATE TYPE post_status AS ENUM ('draft', 'published');
        CREATE TABLE posts (
            id SERIAL PRIMARY KEY,
            title VARCHAR(255) NOT NULL,
            status post_status NOT NULL
        );
    ";
    simulate_tern_generate(
        "postgres",
        &engine,
        v1,
        &migrations_dir,
        &schema_path,
        "v1_enum",
    )
    .await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE TYPE post_status AS ENUM ('draft', 'published');"));

    // V2: Add variant 'archived' to ENUM
    let v2 = "
        CREATE TYPE post_status AS ENUM ('draft', 'published', 'archived');
        CREATE TABLE posts (
            id SERIAL PRIMARY KEY,
            title VARCHAR(255) NOT NULL,
            status post_status NOT NULL
        );
    ";
    simulate_tern_generate(
        "postgres",
        &engine,
        v2,
        &migrations_dir,
        &schema_path,
        "v2_enum_add",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(v2_sql.contains("ALTER TYPE post_status ADD VALUE 'archived';"));

    // V3: Drop table and enum
    let v3 = "";
    simulate_tern_generate(
        "postgres",
        &engine,
        v3,
        &migrations_dir,
        &schema_path,
        "v3_enum_drop",
    )
    .await;
    let v3_sql = get_latest_migration_content(&migrations_dir);
    assert!(v3_sql.contains("DROP TYPE post_status;"));

    let _ = fs::remove_dir_all(base_dir);
}
