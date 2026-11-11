use super::{BoxFuture, DatabaseAdapter};
use crate::config::TernConfig;
use crate::differential_engine::model::{
    clean_ident, should_include_schema, should_include_table_with_schema, NormalizedColumn,
    NormalizedEnum, NormalizedForeignKey, NormalizedFunction, NormalizedIndex, NormalizedSchema,
    NormalizedTable, NormalizedTrigger,
};
use sea_schema::postgres::discovery::SchemaDiscovery as PgDiscovery;
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;

pub struct PostgresAdapter;

impl DatabaseAdapter for PostgresAdapter {
    fn introspect<'a>(
        &'a self,
        db_url: &'a str,
        config: &'a TernConfig,
    ) -> BoxFuture<'a, Result<NormalizedSchema, Box<dyn std::error::Error>>> {
        Box::pin(async move {
            let mut schema = NormalizedSchema::default();
            let pool = PgPool::connect(db_url).await?;

            let schema_rows = sqlx::query(
                "SELECT nspname FROM pg_namespace
                 WHERE nspname NOT LIKE 'pg_%' AND nspname != 'information_schema'
                 ORDER BY nspname",
            )
            .fetch_all(&pool)
            .await?;

            let mut active_schemas = Vec::new();
            for r in schema_rows {
                let nsp: String = r.get("nspname");
                if should_include_schema(&nsp, config) {
                    active_schemas.push(nsp);
                }
            }

            if active_schemas.is_empty() {
                return Ok(schema);
            }

            for cur_schema in active_schemas {
                let live_schema = PgDiscovery::new(pool.clone(), &cur_schema)
                    .discover()
                    .await?;

                // 1. Fetch live indexes
                let idx_rows = sqlx::query(
                    "SELECT tablename, indexname, indexdef FROM pg_indexes WHERE schemaname = $1",
                )
                .bind(&cur_schema)
                .fetch_all(&pool)
                .await?;

                // 2. Fetch live tables
                for t in live_schema.tables {
                    let t_name = t.info.name.to_lowercase();
                    if !should_include_table_with_schema(Some(&cur_schema), &t_name, config) {
                        continue;
                    }

                    let mut cols = BTreeMap::new();
                    for c in t.columns {
                        cols.insert(
                            c.name.to_lowercase(),
                            NormalizedColumn {
                                name: c.name.to_lowercase(),
                                data_type: format!("{:?}", c.col_type),
                                not_null: c.not_null.is_some(),
                            },
                        );
                    }

                    let mut pk_cols = Vec::new();
                    for pk in t.primary_key_constraints {
                        for col in pk.columns {
                            pk_cols.push(col.to_lowercase());
                        }
                    }

                    let mut fks = Vec::new();
                    for fk in t.reference_constraints {
                        fks.push(NormalizedForeignKey {
                            name: Some(fk.name.to_lowercase()),
                            columns: fk.columns.iter().map(|c| c.to_lowercase()).collect(),
                            foreign_table: fk.table.to_lowercase(),
                            referred_columns: fk
                                .foreign_columns
                                .iter()
                                .map(|c| c.to_lowercase())
                                .collect(),
                        });
                    }

                    let mut indexes = Vec::new();
                    for row in &idx_rows {
                        let tbl: String = row.get("tablename");
                        let idx_name: String = row.get("indexname");
                        let idx_def: String = row.get("indexdef");

                        if tbl.eq_ignore_ascii_case(&t_name) && !idx_name.ends_with("_pkey") {
                            if let (Some(start), Some(end)) =
                                (idx_def.rfind('('), idx_def.rfind(')'))
                            {
                                let cols_str = &idx_def[start + 1..end];
                                let idx_cols: Vec<String> =
                                    cols_str.split(',').map(|s| clean_ident(s.trim())).collect();
                                let unique = idx_def.to_uppercase().contains("UNIQUE INDEX");

                                indexes.push(NormalizedIndex {
                                    name: idx_name.to_lowercase(),
                                    columns: idx_cols,
                                    unique,
                                });
                            }
                        }
                    }

                    let table_key = if cur_schema == "public" {
                        t_name.clone()
                    } else {
                        format!("{}.{}", cur_schema, t_name)
                    };

                    schema.tables.insert(
                        table_key,
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

                // 3. Fetch Custom ENUMs
                let enum_rows = sqlx::query(
                    "SELECT t.typname AS name, array_agg(e.enumlabel ORDER BY e.enumsortorder) AS variants
                     FROM pg_type t
                     JOIN pg_enum e ON t.oid = e.enumtypid
                     JOIN pg_namespace n ON n.oid = t.typnamespace
                     WHERE n.nspname = $1
                     GROUP BY t.typname",
                )
                .bind(&cur_schema)
                .fetch_all(&pool)
                .await?;

                for row in enum_rows {
                    let name: String = row.get("name");
                    let variants: Vec<String> = row.get("variants");
                    schema
                        .enums
                        .insert(name.clone(), NormalizedEnum { name, variants });
                }

                // 4. Fetch Functions
                let func_rows = sqlx::query(
                    "SELECT p.proname AS name,
                            pg_get_function_identity_arguments(p.oid) AS args,
                            pg_get_functiondef(p.oid) AS def
                     FROM pg_proc p
                     JOIN pg_namespace n ON p.pronamespace = n.oid
                     WHERE n.nspname = $1",
                )
                .bind(&cur_schema)
                .fetch_all(&pool)
                .await?;

                for row in func_rows {
                    let name: String = row.get("name");
                    let args_raw: String = row.get("args");
                    let definition: String = row.get("def");

                    let mut arg_types = Vec::new();
                    for arg in args_raw.split(',') {
                        let trimmed = arg.trim();
                        if !trimmed.is_empty() {
                            if let Some(dt) = trimmed.split_whitespace().last() {
                                arg_types.push(clean_ident(dt));
                            }
                        }
                    }
                    let signature = format!("{}({})", name.to_lowercase(), arg_types.join(", "));

                    schema.functions.insert(
                        signature.clone(),
                        NormalizedFunction {
                            name,
                            signature,
                            definition,
                        },
                    );
                }

                // 5. Fetch Triggers
                let trig_rows = sqlx::query(
                    "SELECT t.tgname AS name, c.relname AS table_name, pg_get_triggerdef(t.oid) AS def
                     FROM pg_trigger t
                     JOIN pg_class c ON t.tgrelid = c.oid
                     JOIN pg_namespace n ON c.relnamespace = n.oid
                     WHERE n.nspname = $1 AND NOT t.tgisinternal",
                )
                .bind(&cur_schema)
                .fetch_all(&pool)
                .await?;

                for row in trig_rows {
                    let name: String = row.get("name");
                    let table: String = row.get("table_name");
                    let definition: String = row.get("def");

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
            "ALTER TABLE {} DROP CONSTRAINT {}_pkey;",
            table, table
        ))
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
            "ALTER TABLE {} DROP CONSTRAINT {};",
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
        let create_or_replace = if target_func
            .definition
            .contains("CREATE OR REPLACE FUNCTION")
        {
            target_func
                .definition
                .trim()
                .trim_end_matches(';')
                .to_string()
        } else {
            target_func
                .definition
                .replace("CREATE FUNCTION", "CREATE OR REPLACE FUNCTION")
                .trim()
                .trim_end_matches(';')
                .to_string()
        };
        vec![format!("{};", create_or_replace)]
    }

    fn drop_function(&self, signature: &str) -> String {
        format!("DROP FUNCTION {};", signature)
    }

    fn drop_trigger(&self, table: &str, trigger_name: &str) -> String {
        format!("DROP TRIGGER {} ON {};", trigger_name, table)
    }

    fn is_function_same(&self, live_clean: &str, target_clean: &str) -> bool {
        live_clean == target_clean
    }

    fn is_trigger_same(&self, live_clean: &str, target_clean: &str) -> bool {
        live_clean == target_clean
    }
}
