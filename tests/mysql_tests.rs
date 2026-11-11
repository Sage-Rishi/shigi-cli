// tests/mysql_tests.rs
mod common;
mod mysql;

#[tokio::test]
async fn test_all_mysql_entities_sequentially() {
    println!("\n--- Running MySQL Table Entity Tests ---");
    mysql::table_tests::test_mysql_table_entities().await;

    println!("\n--- Running MySQL Foreign Key Entity Tests ---");
    mysql::foreign_key_tests::test_mysql_foreign_key_entities().await;

    println!("\n--- Running MySQL Function Entity Tests ---");
    mysql::function_tests::test_mysql_function_entities().await;

    println!("\n--- Running MySQL Trigger Entity Tests ---");
    mysql::trigger_tests::test_mysql_trigger_entities().await;
}
