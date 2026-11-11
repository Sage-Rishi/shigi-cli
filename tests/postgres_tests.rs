// tests/postgres_tests.rs
mod common;
mod postgres;

#[tokio::test]
async fn test_all_postgres_entities_sequentially() {
    println!("\n--- Running Postgres Table Entity Tests ---");
    postgres::table_tests::test_postgres_table_entities().await;

    println!("\n--- Running Postgres Enum Entity Tests ---");
    postgres::enum_tests::test_postgres_enum_entities().await;

    println!("\n--- Running Postgres Function Entity Tests ---");
    postgres::function_tests::test_postgres_function_entities().await;

    println!("\n--- Running Postgres Trigger Entity Tests ---");
    postgres::trigger_tests::test_postgres_trigger_entities().await;
}
