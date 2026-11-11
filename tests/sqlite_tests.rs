// tests/sqlite_tests.rs
mod common;
mod sqlite;

#[tokio::test]
async fn test_all_sqlite_entities_sequentially() {
    println!("\n--- Running SQLite Table Entity Tests ---");
    sqlite::table_tests::test_sqlite_table_entities().await;

    println!("\n--- Running SQLite Foreign Key Entity Tests ---");
    sqlite::foreign_key_tests::test_sqlite_foreign_key_entities().await;

    println!("\n--- Running SQLite Index Entity Tests ---");
    sqlite::index_tests::test_sqlite_index_entities().await;

    println!("\n--- Running SQLite Trigger Entity Tests ---");
    sqlite::trigger_tests::test_sqlite_trigger_entities().await;
}
