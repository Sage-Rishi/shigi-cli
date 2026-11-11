// tests/mysql/foreign_key_tests.rs
use crate::common::{
    detect_working_container_engine, get_latest_migration_content, simulate_tern_generate,
};
use std::fs;

pub async fn test_mysql_foreign_key_entities() {
    let engine = match detect_working_container_engine().await {
        Some(e) => e,
        None => {
            println!("Skipping MySQL foreign key test: no accessible container engine running.");
            return;
        }
    };

    let base_dir = "tests/temp_mysql_fk_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    let v1 = "
        CREATE TABLE authors (
            id VARCHAR(36) PRIMARY KEY,
            name VARCHAR(100) NOT NULL
        );
        CREATE TABLE books (
            id INT AUTO_INCREMENT PRIMARY KEY,
            author_id VARCHAR(36),
            title VARCHAR(255) NOT NULL,
            FOREIGN KEY (author_id) REFERENCES authors(id)
        );
    ";
    simulate_tern_generate("mysql", &engine, v1, &migrations_dir, &schema_path, "v1_fk").await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE TABLE authors"));
    assert!(v1_sql.contains("CREATE TABLE books"));

    // V2: Add junction table with composite foreign keys
    let v2 = "
        CREATE TABLE authors (
            id VARCHAR(36) PRIMARY KEY,
            name VARCHAR(100) NOT NULL
        );
        CREATE TABLE books (
            id INT AUTO_INCREMENT PRIMARY KEY,
            author_id VARCHAR(36),
            title VARCHAR(255) NOT NULL,
            FOREIGN KEY (author_id) REFERENCES authors(id)
        );
        CREATE TABLE categories (
            id VARCHAR(36) PRIMARY KEY,
            name VARCHAR(50) NOT NULL
        );
        CREATE TABLE book_categories (
            book_id INT NOT NULL,
            category_id VARCHAR(36) NOT NULL,
            PRIMARY KEY (book_id, category_id),
            FOREIGN KEY (book_id) REFERENCES books(id),
            FOREIGN KEY (category_id) REFERENCES categories(id)
        );
    ";
    simulate_tern_generate(
        "mysql",
        &engine,
        v2,
        &migrations_dir,
        &schema_path,
        "v2_junction",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(v2_sql.contains("CREATE TABLE book_categories"));

    let _ = fs::remove_dir_all(base_dir);
}
