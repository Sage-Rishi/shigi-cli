use super::{BoxFuture, DatabaseAdapter};
use crate::config::TernConfig;
use crate::differential_engine::model::{
    should_include_table, NormalizedColumn, NormalizedForeignKey, NormalizedFunction,
    NormalizedIndex, NormalizedSchema, NormalizedTable, NormalizedTrigger,
};
use sea_schema::sqlite::discovery::SchemaDiscovery as SqliteDiscovery;
use sqlx::{Row, SqlitePool};
use std::collections::BTreeMap;

pub struct SqliteAdapter;

impl DatabaseAdapter for SqliteAdapter {
    fn introspect<'a>(
        &'a self,
        db_url: &'a str,
        config: &'a TernConfig,
    ) -> BoxFuture<'a, Result<NormalizedSchema, Box<dyn std::error::Error>>> {
        Box::pin(async move {
            let mut schema = NormalizedSchema::default();
            let pool = SqlitePool::connect(db_url).await?;
            let live_schema = SqliteDiscovery::new(pool.clone()).discover().await?;

            // 1. Fetch tables, columns, PKs
            for t in live_schema.tables {
                let t_name = t.name.to_lowercase();
                if !should_include_table(&t_name, config) {
                    continue;
                }

                let mut cols = BTreeMap::new();
                let mut pk_cols = Vec::new();
                for c in t.columns {
                    let c_name = c.name.to_lowercase();
                    if c.primary_key {
                        pk_cols.push(c_name.clone());
                    }
                    cols.insert(
                        c_name.clone(),
                        NormalizedColumn {
                            name: c_name,
                            data_type: format!("{:?}", c.r#type),
                            not_null: c.not_null || c.primary_key,
                        },
                    );
                }

                // 2. Query SQLite PRAGMA foreign_key_list
                let fk_rows = sqlx::query(&format!("PRAGMA foreign_key_list(\"{}\")", t_name))
                    .fetch_all(&pool)
                    .await?;

                let mut fk_groups: BTreeMap<i64, NormalizedForeignKey> = BTreeMap::new();
                for row in fk_rows {
                    let id: i64 = row.get("id");
                    let ref_table: String = row.get("table");
                    let from_col: String = row.get("from");
                    let to_col: String = row.get("to");

                    let entry = fk_groups.entry(id).or_insert_with(|| NormalizedForeignKey {
                        name: None,
                        columns: Vec::new(),
                        foreign_table: ref_table.to_lowercase(),
                        referred_columns: Vec::new(),
                    });
                    entry.columns.push(from_col.to_lowercase());
                    entry.referred_columns.push(to_col.to_lowercase());
                }
                let fks: Vec<NormalizedForeignKey> = fk_groups.into_values().collect();

                // 3. Query SQLite PRAGMA index_list & index_info
                let idx_rows = sqlx::query(&format!("PRAGMA index_list(\"{}\")", t_name))
                    .fetch_all(&pool)
                    .await?;

                let mut indexes = Vec::new();
                for row in idx_rows {
                    let idx_name: String = row.get("name");
                    let unique: bool = row.get::<i64, _>("unique") != 0;
                    let origin: String = row.get("origin");

                    if origin == "pk" {
                        continue;
                    }

                    let info_rows = sqlx::query(&format!("PRAGMA index_info(\"{}\")", idx_name))
                        .fetch_all(&pool)
                        .await?;
                    let mut idx_cols = Vec::new();
                    for info in info_rows {
                        let col_name: String = info.get("name");
                        idx_cols.push(col_name.to_lowercase());
                    }

                    indexes.push(NormalizedIndex {
                        name: idx_name.to_lowercase(),
                        columns: idx_cols,
                        unique,
                    });
                }

                schema.tables.insert(
                    t_name.clone(),
                    NormalizedTable {
                        name: t_name,
                        columns: cols,
                        primary_key: pk_cols,
                        foreign_keys: fks,
                        indexes,
                        raw_create_sql: None,
                    },
                );
            }

            // 4. Fetch Triggers from sqlite_master
            let trig_rows =
                sqlx::query("SELECT name, tbl_name, sql FROM sqlite_master WHERE type = 'trigger'")
                    .fetch_all(&pool)
                    .await?;

            for row in trig_rows {
                let name: String = row.get("name");
                let table: String = row.get("tbl_name");
                if let Ok(definition) = row.try_get::<String, _>("sql") {
                    let key = format!("{}_{}", table, name);
                    schema.triggers.insert(
                        key,
                        NormalizedTrigger {
                            name,
                            table,
                            definition,
                        },
                    );
                }
            }

            Ok(schema)
        })
    }

    fn alter_column_type(&self, table: &str, col_name: &str, new_type: &str) -> String {
        format!(
            "ALTER TABLE {} ALTER COLUMN {} TYPE {};",
            table, col_name, new_type
        )
    }

    fn drop_primary_key(&self, table: &str) -> Option<String> {
        Some(format!(
            "-- WARNING: Primary key change detected on '{}'. SQLite requires table recreation to alter primary keys.",
            table
        ))
    }

    fn add_primary_key(&self, _table: &str, _columns: &[String]) -> Option<String> {
        None
    }

    fn drop_foreign_key(&self, table: &str, _fk_name: &str, columns: &[String]) -> Option<String> {
        Some(format!(
            "-- WARNING: Foreign key removal on '{}' ({:?}) requires table rebuild in SQLite.",
            table, columns
        ))
    }

    fn add_foreign_key(&self, table: &str, fk: &NormalizedForeignKey) -> Option<String> {
        Some(format!(
            "-- WARNING: Adding foreign key on '{}' ({:?} -> {}) requires table rebuild in SQLite.",
            table, fk.columns, fk.foreign_table
        ))
    }

    fn drop_index(&self, _table: &str, idx_name: &str) -> String {
        format!("DROP INDEX {};", idx_name)
    }

    fn is_index_needed_by_foreign_key(
        &self,
        _live_idx: &NormalizedIndex,
        _target_table: &NormalizedTable,
        _live_table: &NormalizedTable,
    ) -> bool {
        false
    }

    fn modify_function(&self, target_func: &NormalizedFunction) -> Vec<String> {
        vec![format!("{};", target_func.definition)]
    }

    fn drop_function(&self, signature: &str) -> String {
        format!("DROP FUNCTION {};", signature)
    }

    fn drop_trigger(&self, _table: &str, trigger_name: &str) -> String {
        format!("DROP TRIGGER {};", trigger_name)
    }

    fn is_function_same(&self, live_clean: &str, target_clean: &str) -> bool {
        live_clean == target_clean
    }

    fn is_trigger_same(&self, live_clean: &str, target_clean: &str) -> bool {
        live_clean == target_clean
    }
}
