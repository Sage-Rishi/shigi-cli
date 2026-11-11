pub mod mysql;
pub mod postgres;
pub mod sqlite;

use super::model::{
    NormalizedForeignKey, NormalizedFunction, NormalizedIndex, NormalizedSchema, NormalizedTable,
};
use crate::config::TernConfig;
use std::future::Future;
use std::pin::Pin;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait DatabaseAdapter: Send + Sync {
    /// Introspects live database state to extract all existing tables, columns, indexes, FKs, triggers, and functions.
    fn introspect<'a>(
        &'a self,
        db_url: &'a str,
        config: &'a TernConfig,
    ) -> BoxFuture<'a, Result<NormalizedSchema, Box<dyn std::error::Error>>>;

    /// Returns SQL statement to alter a column's data type.
    fn alter_column_type(&self, table: &str, col_name: &str, new_type: &str) -> String;

    /// Returns SQL statement to drop a table's primary key, if supported.
    fn drop_primary_key(&self, table: &str) -> Option<String>;

    /// Returns SQL statement to add a primary key, if supported.
    fn add_primary_key(&self, table: &str, columns: &[String]) -> Option<String>;

    /// Returns SQL statement to drop a foreign key constraint.
    fn drop_foreign_key(&self, table: &str, fk_name: &str, columns: &[String]) -> Option<String>;

    /// Returns SQL statement to add a foreign key constraint.
    fn add_foreign_key(&self, table: &str, fk: &NormalizedForeignKey) -> Option<String>;

    /// Returns SQL statement to drop a secondary index.
    fn drop_index(&self, table: &str, idx_name: &str) -> String;

    /// Checks if a secondary index is implicitly required by an existing foreign key in this dialect.
    fn is_index_needed_by_foreign_key(
        &self,
        live_idx: &NormalizedIndex,
        target_table: &NormalizedTable,
        live_table: &NormalizedTable,
    ) -> bool;

    /// Formats function modification statements (e.g. DROP & CREATE for MySQL, CREATE OR REPLACE for Postgres).
    fn modify_function(&self, target_func: &NormalizedFunction) -> Vec<String>;

    /// Formats statement to drop a function.
    fn drop_function(&self, signature: &str) -> String;

    /// Formats statement to drop a trigger.
    fn drop_trigger(&self, table: &str, trigger_name: &str) -> String;

    /// Normalizes/compares two function definitions for zero-drift checks.
    fn is_function_same(&self, live_clean: &str, target_clean: &str) -> bool;

    /// Normalizes/compares two trigger definitions for zero-drift checks.
    fn is_trigger_same(&self, live_clean: &str, target_clean: &str) -> bool;
}

/// Factory function to return the appropriate database adapter for the given dialect.
pub fn get_adapter(db_type: &str) -> Result<Box<dyn DatabaseAdapter>, Box<dyn std::error::Error>> {
    match db_type.to_lowercase().as_str() {
        "postgres" | "postgresql" => Ok(Box::new(postgres::PostgresAdapter)),
        "mysql" => Ok(Box::new(mysql::MySqlAdapter)),
        "sqlite" => Ok(Box::new(sqlite::SqliteAdapter)),
        _ => Err(format!("Unsupported database dialect: '{}'", db_type).into()),
    }
}
