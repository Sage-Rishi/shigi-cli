// tests/sqlite/foreign_key_tests.rs
use crate::common::{get_latest_migration_content, simulate_tern_generate};
use std::fs;

pub async fn test_sqlite_foreign_key_entities() {
    let base_dir = "tests/temp_sqlite_fk_tests";
    let migrations_dir = format!("{}/migrations", base_dir);
    let schema_path = format!("{}/schema.sql", base_dir);

    let _ = fs::remove_dir_all(base_dir);
    fs::create_dir_all(&migrations_dir).unwrap();

    // V1: 1:M (users -> posts), M:M junction (posts & tags -> post_tags), and self-referential (categories -> categories)
    let v1 = "
        CREATE TABLE users (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL
        );
        CREATE TABLE categories (
            id TEXT PRIMARY KEY,
            parent_id TEXT,
            name TEXT NOT NULL,
            FOREIGN KEY(parent_id) REFERENCES categories(id)
        );
        CREATE TABLE posts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            author_id TEXT NOT NULL,
            category_id TEXT NOT NULL,
            title TEXT NOT NULL,
            FOREIGN KEY(author_id) REFERENCES users(id),
            FOREIGN KEY(category_id) REFERENCES categories(id)
        );
        CREATE TABLE tags (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL
        );
        CREATE TABLE post_tags (
            post_id INTEGER NOT NULL,
            tag_id TEXT NOT NULL,
            PRIMARY KEY (post_id, tag_id),
            FOREIGN KEY(post_id) REFERENCES posts(id),
            FOREIGN KEY(tag_id) REFERENCES tags(id)
        );
    ";
    simulate_tern_generate(
        "sqlite",
        "none",
        v1,
        &migrations_dir,
        &schema_path,
        "v1_fks",
    )
    .await;
    let v1_sql = get_latest_migration_content(&migrations_dir);
    assert!(v1_sql.contains("CREATE TABLE categories"));
    assert!(v1_sql.contains("parent_id") && v1_sql.contains("REFERENCES categories"));
    assert!(v1_sql.contains("CREATE TABLE post_tags"));

    // V2: Add self-referential threaded comments with multiple FKs (post_id, author_id, parent_comment_id)
    let v2 = format!(
        "{}\n{}",
        v1,
        "
        CREATE TABLE comments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            post_id INTEGER NOT NULL,
            author_id TEXT NOT NULL,
            parent_comment_id INTEGER,
            content TEXT NOT NULL,
            FOREIGN KEY(post_id) REFERENCES posts(id),
            FOREIGN KEY(author_id) REFERENCES users(id),
            FOREIGN KEY(parent_comment_id) REFERENCES comments(id)
        );
        "
    );
    simulate_tern_generate(
        "sqlite",
        "none",
        &v2,
        &migrations_dir,
        &schema_path,
        "v2_comments",
    )
    .await;
    let v2_sql = get_latest_migration_content(&migrations_dir);
    assert!(v2_sql.contains("CREATE TABLE comments"));
    assert!(v2_sql.contains("parent_comment_id") && v2_sql.contains("REFERENCES comments"));

    let _ = fs::remove_dir_all(base_dir);
}
