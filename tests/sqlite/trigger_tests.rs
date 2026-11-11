// tests/sqlite/trigger_tests.rs
use crate::common::{get_latest_migration_content, simulate_tern_generate};
use std::fs;

pub async fn test_sqlite_trigger_entities() {
    let base_dir = "tests/temp_sqlite_trigger_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    let v1 = "
        CREATE TABLE items (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            updated_at DATETIME
        );
        CREATE TRIGGER trg_items_update
        AFTER UPDATE ON items
        FOR EACH ROW
        BEGIN
            UPDATE items SET updated_at = CURRENT_TIMESTAMP WHERE id = OLD.id;
        END;
    ";
    simulate_tern_generate(
        "sqlite",
        "none",
        v1,
        &migrations_dir,
        &schema_path,
        "v1_trigger",
    )
    .await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE TRIGGER trg_items_update"));

    // V2: Modify trigger definition
    let v2 = "
        CREATE TABLE items (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            updated_at DATETIME
        );
        CREATE TRIGGER trg_items_update
        AFTER UPDATE ON items
        FOR EACH ROW
        BEGIN
            UPDATE items SET updated_at = '2026-11-11' WHERE id = OLD.id;
        END;
    ";
    simulate_tern_generate(
        "sqlite",
        "none",
        v2,
        &migrations_dir,
        &schema_path,
        "v2_trigger_mod",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(v2_sql.contains("DROP TRIGGER trg_items_update;"));
    assert!(v2_sql.contains("CREATE TRIGGER trg_items_update"));

    // V3: Drop trigger
    let v3 = "
        CREATE TABLE items (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            updated_at DATETIME
        );
    ";
    simulate_tern_generate(
        "sqlite",
        "none",
        v3,
        &migrations_dir,
        &schema_path,
        "v3_trigger_drop",
    )
    .await;
    let v3_sql = get_latest_migration_content(&migrations_dir);
    assert!(v3_sql.contains("DROP TRIGGER trg_items_update;"));

    let _ = fs::remove_dir_all(base_dir);
}
