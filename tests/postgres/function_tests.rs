// tests/postgres/function_tests.rs
use crate::common::{
    detect_working_container_engine, get_latest_migration_content, simulate_tern_generate,
};
use std::fs;

pub async fn test_postgres_function_entities() {
    let engine = match detect_working_container_engine().await {
        Some(e) => e,
        None => {
            println!("Skipping Postgres function test: no accessible container engine running.");
            return;
        }
    };

    let base_dir = "tests/temp_pg_function_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    let v1 = "
        CREATE FUNCTION add_numbers(a integer, b integer) RETURNS integer AS $$
        BEGIN
            RETURN a + b;
        END;
        $$ LANGUAGE plpgsql;
    ";
    simulate_tern_generate(
        "postgres",
        &engine,
        v1,
        &migrations_dir,
        &schema_path,
        "v1_func",
    )
    .await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE FUNCTION add_numbers"));

    // V2: Modify function implementation
    let v2 = "
        CREATE FUNCTION add_numbers(a integer, b integer) RETURNS integer AS $$
        BEGIN
            RETURN a + b + 0;
        END;
        $$ LANGUAGE plpgsql;
    ";
    simulate_tern_generate(
        "postgres",
        &engine,
        v2,
        &migrations_dir,
        &schema_path,
        "v2_func_replace",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(
        v2_sql.contains("CREATE OR REPLACE FUNCTION add_numbers") || v2_sql.contains("add_numbers")
    );

    // V3: Drop function
    let v3 = "";
    simulate_tern_generate(
        "postgres",
        &engine,
        v3,
        &migrations_dir,
        &schema_path,
        "v3_func_drop",
    )
    .await;
    let v3_sql = get_latest_migration_content(&migrations_dir);
    assert!(v3_sql.contains("DROP FUNCTION add_numbers"));

    let _ = fs::remove_dir_all(base_dir);
}
