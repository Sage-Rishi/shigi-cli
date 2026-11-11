// tests/complex_relational_test.rs
mod common;

use common::{
    detect_working_container_engine, get_latest_migration_content, simulate_tern_generate,
};
use std::fs;
use tern::config::{FilterConfig, PathConfig, ProjectConfig, TernConfig};
use tern::db;
use tern::differential_engine;

async fn test_large_complex_sqlite_relational_network() {
    let base_dir = "tests/temp_large_complex_sqlite";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = "tests/schemas/complex_sqlite.sql";

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    let complex_sql = fs::read_to_string(schema_path).expect("Missing complex_sqlite.sql");

    // Phase 1: Generate initial migration from large 17-table relational schema
    simulate_tern_generate(
        "sqlite",
        "none",
        &complex_sql,
        &migrations_dir,
        &format!("{}/schema.sql", base_dir),
        "large_complex_init",
    )
    .await;

    let init_sql = get_latest_migration_content(&migrations_dir);

    // Assert that all primary, one-to-many, many-to-one, and self-referential tables were generated
    let expected_tables = [
        "CREATE TABLE organizations",
        "CREATE TABLE roles",
        "CREATE TABLE users",
        "CREATE TABLE user_profiles",
        "CREATE TABLE user_organizations",
        "CREATE TABLE categories",
        "CREATE TABLE tags",
        "CREATE TABLE posts",
        "CREATE TABLE post_tags",
        "CREATE TABLE comments",
        "CREATE TABLE comment_reactions",
        "CREATE TABLE products",
        "CREATE TABLE product_variants",
        "CREATE TABLE orders",
        "CREATE TABLE order_items",
        "CREATE TABLE payments",
        "CREATE TABLE audit_logs",
    ];

    for tbl in &expected_tables {
        assert!(
            init_sql.contains(tbl),
            "Generated migration missing table: {}",
            tbl
        );
    }

    // Assert FK relationships are present
    assert!(init_sql.contains("REFERENCES users"));
    assert!(init_sql.contains("REFERENCES organizations"));
    assert!(init_sql.contains("REFERENCES categories"));
    assert!(init_sql.contains("REFERENCES posts"));
    assert!(init_sql.contains("REFERENCES comments"));
    assert!(init_sql.contains("REFERENCES products"));
    assert!(init_sql.contains("REFERENCES orders"));
    assert!(init_sql.contains("REFERENCES product_variants"));

    // Phase 2: Verify zero drift by replaying all migrations and generating diff against unchanged schema
    let config = TernConfig {
        project: ProjectConfig {
            db_type: "sqlite".to_string(),
            engine: "none".to_string(),
            target_url: None,
        },
        paths: PathConfig {
            schema_path: format!("{}/schema.sql", base_dir),
            migrations_dir: migrations_dir.clone(),
        },
        filter: FilterConfig {
            include_tables: vec![],
            exclude_tables: vec!["_tern_migrations".to_string()],
        },
    };

    let (pool, db_url) = db::connect("sqlite").await.unwrap();
    db::apply_migrations(&pool, &migrations_dir).await.unwrap();

    let diff_result =
        differential_engine::generate_diff(&config, &db_url, "verify_zero_drift").await;
    assert!(
        diff_result.is_ok(),
        "Diff against replayed schema failed: {:?}",
        diff_result
    );

    let _ = fs::remove_file(db_url.strip_prefix("sqlite://").unwrap_or("tern_shadow.db"));
    let _ = fs::remove_dir_all(base_dir);
}

async fn test_large_complex_postgres_relational_network() {
    let engine = match detect_working_container_engine().await {
        Some(e) => e,
        None => {
            println!(
                "Skipping Postgres large complex test: no accessible container engine running."
            );
            return;
        }
    };

    let base_dir = "tests/temp_large_complex_postgres";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = "tests/schemas/complex_postgres.sql";

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    let complex_sql = fs::read_to_string(schema_path).expect("Missing complex_postgres.sql");

    simulate_tern_generate(
        "postgres",
        &engine,
        &complex_sql,
        &migrations_dir,
        &format!("{}/schema.sql", base_dir),
        "pg_complex_init",
    )
    .await;

    let init_sql = get_latest_migration_content(&migrations_dir);
    assert!(init_sql.contains("CREATE TYPE order_status"));
    assert!(init_sql.contains("CREATE TYPE payment_status"));
    assert!(init_sql.contains("CREATE TABLE organizations"));
    assert!(init_sql.contains("CREATE TABLE orders"));
    assert!(init_sql.contains("CREATE TABLE order_items"));
    assert!(init_sql.contains("CREATE FUNCTION set_updated_at_timestamp"));
    assert!(init_sql.contains("CREATE TRIGGER trg_posts_updated_at"));

    let _ = fs::remove_dir_all(base_dir);
}

async fn test_large_complex_mysql_relational_network() {
    let engine = match detect_working_container_engine().await {
        Some(e) => e,
        None => {
            println!("Skipping MySQL large complex test: no accessible container engine running.");
            return;
        }
    };

    let base_dir = "tests/temp_large_complex_mysql";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = "tests/schemas/complex_mysql.sql";

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    let complex_sql = fs::read_to_string(schema_path).expect("Missing complex_mysql.sql");

    simulate_tern_generate(
        "mysql",
        &engine,
        &complex_sql,
        &migrations_dir,
        &format!("{}/schema.sql", base_dir),
        "mysql_complex_init",
    )
    .await;

    let init_sql = get_latest_migration_content(&migrations_dir);
    assert!(init_sql.contains("CREATE TABLE organizations"));
    assert!(init_sql.contains("CREATE TABLE orders"));
    assert!(init_sql.contains("CREATE TABLE order_items"));
    assert!(init_sql.contains("CREATE FUNCTION get_schema_version"));
    assert!(init_sql.contains("CREATE TRIGGER trg_posts_before_update"));

    let _ = fs::remove_dir_all(base_dir);
}

#[tokio::test]
async fn test_all_complex_relational_networks_sequentially() {
    println!("\n--- Running Large Complex Relational Network: SQLite ---");
    test_large_complex_sqlite_relational_network().await;

    println!("\n--- Running Large Complex Relational Network: Postgres ---");
    test_large_complex_postgres_relational_network().await;

    println!("\n--- Running Large Complex Relational Network: MySQL ---");
    test_large_complex_mysql_relational_network().await;
}
