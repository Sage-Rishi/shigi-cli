use super::{BoxFuture, DatabaseAdapter};
use crate::config::TernConfig;
use crate::differential_engine::model::{
    should_include_schema, should_include_table_with_schema, NormalizedColumn,
    NormalizedForeignKey, NormalizedFunction, NormalizedIndex, NormalizedSchema, NormalizedTable,
    NormalizedTrigger,
};
use sea_schema::mysql::discovery::SchemaDiscovery as MySqlDiscovery;
use sqlx::{MySqlPool, Row};
use std::collections::{BTreeMap, BTreeSet};

pub struct MySqlAdapter;

impl DatabaseAdapter for MySqlAdapter {
    fn introspect<'a>(
        &'a self,
        db_url: &'a str,
        config: &'a TernConfig,
    ) -> BoxFuture<'a, Result<NormalizedSchema, Box<dyn std::error::Error>>> {
        Box::pin(async move {
            let mut schema = NormalizedSchema::default();
            let db_name = "tern_db";
            if !should_include_schema(db_name, config) {
                return Ok(schema);
            }

            let pool = MySqlPool::connect(db_url).await?;
            let live_schema = MySqlDiscovery::new(pool.clone(), db_name)
                .discover()
                .await?;

            // 1. Fetch live tables, columns, indexes, and FKs
            for t in live_schema.tables {
                let t_name = t.info.name.to_lowercase();
                if !should_include_table_with_schema(Some(db_name), &t_name, config) {
                    continue;
                }

                let mut cols = BTreeMap::new();
                for c in t.columns {
                    cols.insert(
                        c.name.to_lowercase(),
                        NormalizedColumn {
                            name: c.name.to_lowercase(),
                            data_type: format!("{:?}", c.col_type),
                            not_null: !c.null,
                        },
                    );
                }

                let mut pk_cols = Vec::new();
                let mut indexes = Vec::new();
                let fk_names: BTreeSet<String> = t
                    .foreign_keys
                    .iter()
                    .map(|fk| fk.name.to_lowercase())
                    .collect();

                let fk_col_sets: Vec<Vec<String>> = t
                    .foreign_keys
                    .iter()
                    .map(|fk| fk.columns.iter().map(|c| c.to_lowercase()).collect())
                    .collect();

                for idx in t.indexes {
                    let idx_name = idx.name.to_lowercase();
                    let idx_cols: Vec<String> =
                        idx.parts.iter().map(|p| p.column.to_lowercase()).collect();

                    if idx_name == "primary" {
                        pk_cols = idx_cols;
                    } else if !fk_names.contains(&idx_name)
                        && !(!idx.unique && fk_col_sets.contains(&idx_cols))
                    {
                        indexes.push(NormalizedIndex {
                            name: idx_name,
                            columns: idx_cols,
                            unique: idx.unique,
                        });
                    }
                }

                let mut fks = Vec::new();
                for fk in t.foreign_keys {
                    fks.push(NormalizedForeignKey {
                        name: Some(fk.name.to_lowercase()),
                        columns: fk.columns.iter().map(|c| c.to_lowercase()).collect(),
                        foreign_table: fk.referenced_table.to_lowercase(),
                        referred_columns: fk
                            .referenced_columns
                            .iter()
                            .map(|c| c.to_lowercase())
                            .collect(),
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

            // 2. Fetch Functions (Routines)
            let func_rows = sqlx::query(
                "SELECT CONVERT(ROUTINE_NAME USING utf8mb4) AS name,
                        CONVERT(ROUTINE_DEFINITION USING utf8mb4) AS def
                 FROM information_schema.ROUTINES
                 WHERE ROUTINE_SCHEMA = 'tern_db' AND ROUTINE_TYPE = 'FUNCTION'",
            )
            .fetch_all(&pool)
            .await?;

            for row in func_rows {
                let name: String = row
                    .try_get("name")
                    .ok()
                    .or_else(|| {
                        row.try_get::<Vec<u8>, _>("name")
                            .ok()
                            .map(|b| String::from_utf8_lossy(&b).to_string())
                    })
                    .unwrap_or_default();

                let def_opt: Option<String> = row
                    .try_get("def")
                    .ok()
                    .or_else(|| {
                        row.try_get::<Vec<u8>, _>("def")
                            .ok()
                            .map(|b| String::from_utf8_lossy(&b).to_string())
                    })
                    .or_else(|| {
                        row.try_get::<Option<Vec<u8>>, _>("def")
                            .ok()
                            .flatten()
                            .map(|b| String::from_utf8_lossy(&b).to_string())
                    });
                let definition = def_opt.unwrap_or_default();

                schema.functions.insert(
                    name.clone(),
                    NormalizedFunction {
                        name: name.clone(),
                        signature: name,
                        definition,
                    },
                );
            }

            // 3. Fetch Triggers
            let trig_rows = sqlx::query(
                "SELECT CONVERT(TRIGGER_NAME USING utf8mb4) AS name,
                        CONVERT(EVENT_OBJECT_TABLE USING utf8mb4) AS table_name,
                        CONVERT(ACTION_STATEMENT USING utf8mb4) AS def
                 FROM information_schema.TRIGGERS
                 WHERE TRIGGER_SCHEMA = 'tern_db'",
            )
            .fetch_all(&pool)
            .await?;

            for row in trig_rows {
                let name: String = row
                    .try_get("name")
                    .ok()
                    .or_else(|| {
                        row.try_get::<Vec<u8>, _>("name")
                            .ok()
                            .map(|b| String::from_utf8_lossy(&b).to_string())
                    })
                    .unwrap_or_default();

                let table: String = row
                    .try_get("table_name")
                    .ok()
                    .or_else(|| {
                        row.try_get::<Vec<u8>, _>("table_name")
                            .ok()
                            .map(|b| String::from_utf8_lossy(&b).to_string())
                    })
                    .unwrap_or_default();

                let def_opt: Option<String> = row
                    .try_get("def")
                    .ok()
                    .or_else(|| {
                        row.try_get::<Vec<u8>, _>("def")
                            .ok()
                            .map(|b| String::from_utf8_lossy(&b).to_string())
                    })
                    .or_else(|| {
                        row.try_get::<Option<Vec<u8>>, _>("def")
                            .ok()
                            .flatten()
                            .map(|b| String::from_utf8_lossy(&b).to_string())
                    });
                let definition = def_opt.unwrap_or_default();

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

            Ok(schema)
        })
    }

    fn alter_column_type(&self, table: &str, col_name: &str, new_type: &str) -> String {
        format!(
            "ALTER TABLE {} MODIFY COLUMN {} {};",
            table, col_name, new_type
        )
    }

    fn drop_primary_key(&self, table: &str) -> Option<String> {
        Some(format!("ALTER TABLE {} DROP PRIMARY KEY;", table))
    }

    fn add_primary_key(&self, table: &str, columns: &[String]) -> Option<String> {
        Some(format!(
            "ALTER TABLE {} ADD PRIMARY KEY ({});",
            table,
            columns.join(", ")
        ))
    }

    fn drop_foreign_key(&self, table: &str, fk_name: &str, _columns: &[String]) -> Option<String> {
        Some(format!(
            "ALTER TABLE {} DROP FOREIGN KEY {};",
            table, fk_name
        ))
    }

    fn add_foreign_key(&self, table: &str, fk: &NormalizedForeignKey) -> Option<String> {
        let fk_name = fk
            .name
            .clone()
            .unwrap_or_else(|| format!("fk_{}_{}", table, fk.columns.join("_")));
        Some(format!(
            "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({});",
            table,
            fk_name,
            fk.columns.join(", "),
            fk.foreign_table,
            fk.referred_columns.join(", ")
        ))
    }

    fn drop_index(&self, table: &str, idx_name: &str) -> String {
        format!("DROP INDEX {} ON {};", idx_name, table)
    }

    fn is_index_needed_by_foreign_key(
        &self,
        live_idx: &NormalizedIndex,
        target_table: &NormalizedTable,
        live_table: &NormalizedTable,
    ) -> bool {
        target_table
            .foreign_keys
            .iter()
            .any(|fk| fk.columns == live_idx.columns)
            || live_table
                .foreign_keys
                .iter()
                .any(|fk| fk.columns == live_idx.columns)
    }

    fn modify_function(&self, target_func: &NormalizedFunction) -> Vec<String> {
        vec![
            format!("DROP FUNCTION IF EXISTS {};", target_func.name),
            format!("{};", target_func.definition.trim().trim_end_matches(';')),
        ]
    }

    fn drop_function(&self, signature: &str) -> String {
        format!("DROP FUNCTION {};", signature)
    }

    fn drop_trigger(&self, _table: &str, trigger_name: &str) -> String {
        format!("DROP TRIGGER IF EXISTS {};", trigger_name)
    }

    fn is_function_same(&self, live_clean: &str, target_clean: &str) -> bool {
        if !live_clean.is_empty() {
            target_clean.contains(live_clean)
        } else {
            live_clean == target_clean
        }
    }

    fn is_trigger_same(&self, live_clean: &str, target_clean: &str) -> bool {
        if !live_clean.is_empty() {
            target_clean.contains(live_clean)
        } else {
            live_clean == target_clean
        }
    }
}
