// tests/mysql/trigger_tests.rs
use crate::common::{
    detect_working_container_engine, get_latest_migration_content, simulate_tern_generate,
};
use std::fs;

pub async fn test_mysql_trigger_entities() {
    let engine = match detect_working_container_engine().await {
        Some(e) => e,
        None => {
            println!("Skipping MySQL trigger test: no accessible container engine running.");
            return;
        }
    };

    let base_dir = "tests/temp_mysql_trigger_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    let v1 = "
        CREATE TABLE posts (
            id INT AUTO_INCREMENT PRIMARY KEY,
            title VARCHAR(255) NOT NULL,
            view_count INT DEFAULT 0
        );

        CREATE TRIGGER before_post_update
        BEFORE UPDATE ON posts
        FOR EACH ROW
        BEGIN
            SET NEW.view_count = OLD.view_count;
        END;
    ";
    simulate_tern_generate(
        "mysql",
        &engine,
        v1,
        &migrations_dir,
        &schema_path,
        "v1_trig",
    )
    .await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE TRIGGER before_post_update"));

    // V2: Modify trigger
    let v2 = "
        CREATE TABLE posts (
            id INT AUTO_INCREMENT PRIMARY KEY,
            title VARCHAR(255) NOT NULL,
            view_count INT DEFAULT 0
        );

        CREATE TRIGGER before_post_update
        BEFORE UPDATE ON posts
        FOR EACH ROW
        BEGIN
            SET NEW.view_count = OLD.view_count + 1;
        END;
    ";
    simulate_tern_generate(
        "mysql",
        &engine,
        v2,
        &migrations_dir,
        &schema_path,
        "v2_trig_mod",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(
        v2_sql.contains("DROP TRIGGER IF EXISTS before_post_update;")
            && v2_sql.contains("CREATE TRIGGER before_post_update")
    );

    // V3: Drop trigger
    let v3 = "";
    simulate_tern_generate(
        "mysql",
        &engine,
        v3,
        &migrations_dir,
        &schema_path,
        "v3_trig_drop",
    )
    .await;
    let v3_sql = get_latest_migration_content(&migrations_dir);
    assert!(v3_sql.contains("DROP TRIGGER IF EXISTS before_post_update;"));

    let _ = fs::remove_dir_all(base_dir);
}
