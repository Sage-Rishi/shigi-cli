pub mod adapters;
pub mod model;
pub mod parser;
pub mod topological;

pub use model::{
    NormalizedColumn, NormalizedEnum, NormalizedForeignKey, NormalizedFunction, NormalizedIndex,
    NormalizedSchema, NormalizedTable, NormalizedTrigger,
};

use crate::config::{write_lockfile, TernConfig};
use adapters::get_adapter;
use chrono::Utc;
use model::normalize_type;
use parser::parse_target_schema;
use std::fs;
use std::path::Path;
use topological::topological_sort;

pub async fn generate_diff(
    config: &TernConfig,
    db_url: &str,
    migration_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let db_type = config.project.db_type.to_lowercase();
    let adapter = get_adapter(&db_type)?;

    // 1. Introspect Live Database State via Dialect Adapter
    let live_schema = adapter.introspect(db_url, config).await?;
    println!(
        "-> [Live State] Found {} table(s), {} enum(s), {} function(s), {} trigger(s).",
        live_schema.tables.len(),
        live_schema.enums.len(),
        live_schema.functions.len(),
        live_schema.triggers.len()
    );

    // 2. Parse Target Declarative Schema Files
    let target_schema = parse_target_schema(&db_type, &config.paths.schema_path, config)?;
    println!(
        "-> [Target State] Found {} table(s), {} enum(s), {} function(s), {} trigger(s).",
        target_schema.tables.len(),
        target_schema.enums.len(),
        target_schema.functions.len(),
        target_schema.triggers.len()
    );

    // 3. Compute SQL Diff Statements
    let mut diff_statements: Vec<String> = Vec::new();

    // --- A. ENUMS (e.g. PostgreSQL) ---
    for (enum_name, target_enum) in &target_schema.enums {
        match live_schema.enums.get(enum_name) {
            None => {
                diff_statements.push(format!(
                    "CREATE TYPE {} AS ENUM ('{}');",
                    enum_name,
                    target_enum.variants.join("', '")
                ));
            }
            Some(live_enum) if live_enum.variants != target_enum.variants => {
                for variant in &target_enum.variants {
                    if !live_enum.variants.contains(variant) {
                        diff_statements
                            .push(format!("ALTER TYPE {} ADD VALUE '{}';", enum_name, variant));
                    }
                }
            }
            _ => {}
        }
    }

    for enum_name in live_schema.enums.keys() {
        if !target_schema.enums.contains_key(enum_name) {
            diff_statements.push(format!("DROP TYPE {};", enum_name));
        }
    }

    // --- B. TABLES & COLUMNS (Topologically ordered for creation) ---
    let sorted_target_tables = topological_sort(&target_schema.tables);
    for table_name in &sorted_target_tables {
        let target_table = target_schema.tables.get(table_name).unwrap();
        match live_schema.tables.get(table_name) {
            None => {
                // New Table: Emit CREATE TABLE and standalone CREATE INDEX statements
                if let Some(raw_sql) = &target_table.raw_create_sql {
                    diff_statements.push(format!("{};", raw_sql));
                }
                for idx in &target_table.indexes {
                    let unique_kw = if idx.unique { "UNIQUE " } else { "" };
                    diff_statements.push(format!(
                        "CREATE {}INDEX {} ON {} ({});",
                        unique_kw,
                        idx.name,
                        table_name,
                        idx.columns.join(", ")
                    ));
                }
            }
            Some(live_table) => {
                // Column Diffs
                for (col_name, target_col) in &target_table.columns {
                    match live_table.columns.get(col_name) {
                        None => {
                            let null_str = if target_col.not_null { " NOT NULL" } else { "" };
                            diff_statements.push(format!(
                                "ALTER TABLE {} ADD COLUMN {} {}{};",
                                table_name, target_col.name, target_col.data_type, null_str
                            ));
                        }
                        Some(live_col) => {
                            if normalize_type(&live_col.data_type)
                                != normalize_type(&target_col.data_type)
                            {
                                diff_statements.push(adapter.alter_column_type(
                                    table_name,
                                    &target_col.name,
                                    &target_col.data_type,
                                ));
                            }
                        }
                    }
                }

                for col_name in live_table.columns.keys() {
                    if !target_table.columns.contains_key(col_name) {
                        diff_statements.push(format!(
                            "ALTER TABLE {} DROP COLUMN {};",
                            table_name, col_name
                        ));
                    }
                }

                // Primary Key Diffs
                if live_table.primary_key != target_table.primary_key {
                    if let Some(drop_pk) = adapter.drop_primary_key(table_name) {
                        diff_statements.push(drop_pk);
                    }
                    if !target_table.primary_key.is_empty() {
                        if let Some(add_pk) =
                            adapter.add_primary_key(table_name, &target_table.primary_key)
                        {
                            diff_statements.push(add_pk);
                        }
                    }
                }

                // Foreign Key Diffs
                for live_fk in &live_table.foreign_keys {
                    let exists_in_target = target_table.foreign_keys.iter().any(|tfk| {
                        tfk.columns == live_fk.columns
                            && tfk.foreign_table == live_fk.foreign_table
                            && tfk.referred_columns == live_fk.referred_columns
                    });

                    if !exists_in_target {
                        let fk_name = live_fk.name.clone().unwrap_or_else(|| {
                            format!("fk_{}_{}", table_name, live_fk.columns.join("_"))
                        });
                        if let Some(drop_fk) =
                            adapter.drop_foreign_key(table_name, &fk_name, &live_fk.columns)
                        {
                            diff_statements.push(drop_fk);
                        }
                    }
                }

                for target_fk in &target_table.foreign_keys {
                    let exists_in_live = live_table.foreign_keys.iter().any(|lfk| {
                        lfk.columns == target_fk.columns
                            && lfk.foreign_table == target_fk.foreign_table
                            && lfk.referred_columns == target_fk.referred_columns
                    });

                    if !exists_in_live {
                        if let Some(add_fk) = adapter.add_foreign_key(table_name, target_fk) {
                            diff_statements.push(add_fk);
                        }
                    }
                }

                // Secondary Index Diffs
                for live_idx in &live_table.indexes {
                    let exists_in_target = target_table.indexes.iter().any(|tidx| {
                        tidx.columns == live_idx.columns && tidx.unique == live_idx.unique
                    });

                    if !exists_in_target
                        && !adapter.is_index_needed_by_foreign_key(
                            live_idx,
                            target_table,
                            live_table,
                        )
                    {
                        diff_statements.push(adapter.drop_index(table_name, &live_idx.name));
                    }
                }

                for target_idx in &target_table.indexes {
                    let exists_in_live = live_table.indexes.iter().any(|lidx| {
                        lidx.columns == target_idx.columns && lidx.unique == target_idx.unique
                    });

                    if !exists_in_live {
                        let unique_kw = if target_idx.unique { "UNIQUE " } else { "" };
                        diff_statements.push(format!(
                            "CREATE {}INDEX {} ON {} ({});",
                            unique_kw,
                            target_idx.name,
                            table_name,
                            target_idx.columns.join(", ")
                        ));
                    }
                }
            }
        }
    }

    // Dropped Tables: Must be dropped in reverse topological order
    let sorted_live_tables = topological_sort(&live_schema.tables);
    for table_name in sorted_live_tables.iter().rev() {
        if !target_schema.tables.contains_key(table_name) {
            diff_statements.push(format!("DROP TABLE {};", table_name));
        }
    }

    // --- C. FUNCTIONS ---
    for (signature, target_func) in &target_schema.functions {
        match live_schema.functions.get(signature) {
            None => {
                diff_statements.push(format!("{};", target_func.definition));
            }
            Some(live_func) => {
                let live_clean = live_func
                    .definition
                    .replace(|c: char| c.is_whitespace(), "");
                let target_clean = target_func
                    .definition
                    .replace(|c: char| c.is_whitespace(), "");

                if !adapter.is_function_same(&live_clean, &target_clean) {
                    diff_statements.extend(adapter.modify_function(target_func));
                }
            }
        }
    }

    for signature in live_schema.functions.keys() {
        if !target_schema.functions.contains_key(signature) {
            diff_statements.push(adapter.drop_function(signature));
        }
    }

    // --- D. TRIGGERS ---
    for (key, target_trigger) in &target_schema.triggers {
        match live_schema.triggers.get(key) {
            None => {
                diff_statements.push(format!(
                    "{};",
                    target_trigger.definition.trim().trim_end_matches(';')
                ));
            }
            Some(live_trigger) => {
                let live_clean = live_trigger
                    .definition
                    .replace(|c: char| c.is_whitespace(), "");
                let target_clean = target_trigger
                    .definition
                    .replace(|c: char| c.is_whitespace(), "");

                if !adapter.is_trigger_same(&live_clean, &target_clean) {
                    diff_statements
                        .push(adapter.drop_trigger(&target_trigger.table, &target_trigger.name));
                    diff_statements.push(format!(
                        "{};",
                        target_trigger.definition.trim().trim_end_matches(';')
                    ));
                }
            }
        }
    }

    for (key, trigger) in &live_schema.triggers {
        if !target_schema.triggers.contains_key(key) {
            diff_statements.push(adapter.drop_trigger(&trigger.table, &trigger.name));
        }
    }

    // --- E. Output Migration Script & Update Lockfile ---
    if diff_statements.is_empty() {
        println!("-> Database schema is completely in sync! No migration generated.");
        return Ok(());
    }

    let timestamp = Utc::now().format("%Y%m%d%H%M%S");
    let filename = format!("{}_{}.sql", timestamp, migration_name);
    let file_path = Path::new(&config.paths.migrations_dir).join(&filename);

    let mut file_content = format!("-- Generated by Tern on {}\n\n", Utc::now().to_rfc3339());
    file_content.push_str(&diff_statements.join("\n\n"));
    file_content.push('\n');

    fs::write(&file_path, file_content)?;
    println!("-> Generated new migration file: {}", file_path.display());

    write_lockfile(&config.paths.migrations_dir)?;
    println!(
        "-> Updated migration hash lockfile ({}/tern.lock).",
        config.paths.migrations_dir
    );

    Ok(())
}
