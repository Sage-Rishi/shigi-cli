// tests/postgres/trigger_tests.rs
use crate::common::{
    detect_working_container_engine, get_latest_migration_content, simulate_tern_generate,
};
use std::fs;

pub async fn test_postgres_trigger_entities() {
    let engine = match detect_working_container_engine().await {
        Some(e) => e,
        None => {
            println!("Skipping Postgres trigger test: no accessible container engine running.");
            return;
        }
    };

    let base_dir = "tests/temp_pg_trigger_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    let v1 = "
        CREATE TABLE posts (
            id SERIAL PRIMARY KEY,
            title VARCHAR(255) NOT NULL,
            updated_at TIMESTAMP
        );

        CREATE FUNCTION update_timestamp() RETURNS trigger AS $$
        BEGIN
            NEW.updated_at = NOW();
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;

        CREATE TRIGGER trg_posts_timestamp
        BEFORE UPDATE ON posts
        FOR EACH ROW EXECUTE FUNCTION update_timestamp();
    ";
    simulate_tern_generate(
        "postgres",
        &engine,
        v1,
        &migrations_dir,
        &schema_path,
        "v1_trig",
    )
    .await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE TRIGGER trg_posts_timestamp"));

    // V2: Modify trigger
    let v2 = "
        CREATE TABLE posts (
            id SERIAL PRIMARY KEY,
            title VARCHAR(255) NOT NULL,
            updated_at TIMESTAMP
        );

        CREATE FUNCTION update_timestamp() RETURNS trigger AS $$
        BEGIN
            NEW.updated_at = CLOCK_TIMESTAMP();
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;

        CREATE TRIGGER trg_posts_timestamp
        BEFORE INSERT OR UPDATE ON posts
        FOR EACH ROW EXECUTE FUNCTION update_timestamp();
    ";
    simulate_tern_generate(
        "postgres",
        &engine,
        v2,
        &migrations_dir,
        &schema_path,
        "v2_trig_mod",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(
        v2_sql.contains("DROP TRIGGER trg_posts_timestamp ON posts;")
            || v2_sql.contains("trg_posts_timestamp")
    );

    // V3: Drop trigger
    let v3 = "";
    simulate_tern_generate(
        "postgres",
        &engine,
        v3,
        &migrations_dir,
        &schema_path,
        "v3_trig_drop",
    )
    .await;
    let v3_sql = get_latest_migration_content(&migrations_dir);
    assert!(v3_sql.contains("DROP TRIGGER trg_posts_timestamp ON posts;"));

    let _ = fs::remove_dir_all(base_dir);
}
