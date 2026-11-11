// tests/mysql/function_tests.rs
use crate::common::{
    detect_working_container_engine, get_latest_migration_content, simulate_tern_generate,
};
use std::fs;

pub async fn test_mysql_function_entities() {
    let engine = match detect_working_container_engine().await {
        Some(e) => e,
        None => {
            println!("Skipping MySQL function test: no accessible container engine running.");
            return;
        }
    };

    let base_dir = "tests/temp_mysql_function_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    let v1 = "
        CREATE FUNCTION get_version() RETURNS VARCHAR(10) DETERMINISTIC
        BEGIN
            RETURN '1.0';
        END;
    ";
    simulate_tern_generate(
        "mysql",
        &engine,
        v1,
        &migrations_dir,
        &schema_path,
        "v1_func",
    )
    .await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE FUNCTION get_version"));

    // V2: Modify function
    let v2 = "
        CREATE FUNCTION get_version() RETURNS VARCHAR(10) DETERMINISTIC
        BEGIN
            RETURN '2.0';
        END;
    ";
    simulate_tern_generate(
        "mysql",
        &engine,
        v2,
        &migrations_dir,
        &schema_path,
        "v2_func_mod",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(
        v2_sql.contains("DROP FUNCTION IF EXISTS get_version")
            && v2_sql.contains("CREATE FUNCTION get_version")
    );

    // V3: Drop function
    let v3 = "";
    simulate_tern_generate(
        "mysql",
        &engine,
        v3,
        &migrations_dir,
        &schema_path,
        "v3_func_drop",
    )
    .await;
    let v3_sql = get_latest_migration_content(&migrations_dir);
    assert!(v3_sql.contains("DROP FUNCTION get_version;"));

    let _ = fs::remove_dir_all(base_dir);
}
