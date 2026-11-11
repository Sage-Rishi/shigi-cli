use crate::config::{write_lockfile, TernConfig};
use chrono::Utc;
use sea_schema::mysql::discovery::SchemaDiscovery as MySqlDiscovery;
use sea_schema::postgres::discovery::SchemaDiscovery as PgDiscovery;
use sea_schema::sqlite::discovery::SchemaDiscovery as SqliteDiscovery;
use sqlparser::ast::{ColumnOption, ObjectName, Statement, TableConstraint};
use sqlparser::dialect::{MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;
use sqlx::{MySqlPool, PgPool, Row, SqlitePool};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
struct NormalizedColumn {
    name: String,
    data_type: String,
    not_null: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NormalizedForeignKey {
    name: Option<String>,
    columns: Vec<String>,
    foreign_table: String,
    referred_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NormalizedIndex {
    name: String,
    columns: Vec<String>,
    unique: bool,
}

#[derive(Debug, Clone)]
struct NormalizedTable {
    #[allow(dead_code)]
    name: String,
    columns: BTreeMap<String, NormalizedColumn>,
    primary_key: Vec<String>,
    foreign_keys: Vec<NormalizedForeignKey>,
    indexes: Vec<NormalizedIndex>,
    raw_create_sql: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedEnum {
    name: String,
    variants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedTrigger {
    name: String,
    table: String,
    definition: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedFunction {
    name: String,
    signature: String, // e.g., "add(integer, integer)"
    definition: String,
}

#[derive(Debug, Clone, Default)]
struct NormalizedSchema {
    tables: BTreeMap<String, NormalizedTable>,
    enums: BTreeMap<String, NormalizedEnum>,
    triggers: BTreeMap<String, NormalizedTrigger>,
    functions: BTreeMap<String, NormalizedFunction>,
}

pub async fn generate_diff(
    config: &TernConfig,
    db_url: &str,
    migration_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let db_type = config.project.db_type.to_lowercase();

    // 1. Introspect Live Database State
    let live_schema = introspect_live_db(&db_type, db_url, config).await?;
    println!(
        "-> [Live State] Found {} table(s), {} enum(s), {} function(s), {} trigger(s).",
        live_schema.tables.len(),
        live_schema.enums.len(),
        live_schema.functions.len(),
        live_schema.triggers.len()
    );

    // 2. Parse Target Declarative Schema
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

    // --- ENUMS (PostgreSQL Only) ---
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
                // If variants changed, we must add the new values.
                // Note: Postgres does not allow dropping enum values without a type recreation.
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

    // Check for Dropped Enums
    for enum_name in live_schema.enums.keys() {
        if !target_schema.enums.contains_key(enum_name) {
            diff_statements.push(format!("DROP TYPE {};", enum_name));
        }
    }

    // --- TABLES & COLUMNS ---
    let sorted_target_tables = topological_sort(&target_schema.tables);
    for table_name in &sorted_target_tables {
        let target_table = target_schema.tables.get(table_name).unwrap();
        match live_schema.tables.get(table_name) {
            None => {
                // New Table: Emit CREATE TABLE and any standalone CREATE INDEX statements
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
                // A. Column Diffs
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
                                let alter_stmt = match db_type.as_str() {
                                    "mysql" => format!(
                                        "ALTER TABLE {} MODIFY COLUMN {} {};",
                                        table_name, target_col.name, target_col.data_type
                                    ),
                                    _ => format!(
                                        "ALTER TABLE {} ALTER COLUMN {} TYPE {};",
                                        table_name, target_col.name, target_col.data_type
                                    ),
                                };
                                diff_statements.push(alter_stmt);
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

                // B. Primary Key Diffs
                if live_table.primary_key != target_table.primary_key {
                    if db_type == "sqlite" {
                        diff_statements.push(format!(
                                "-- WARNING: Primary key change detected on '{}'. SQLite requires table recreation to alter primary keys.",
                                table_name
                            ));
                    } else {
                        if !live_table.primary_key.is_empty() {
                            let drop_pk = match db_type.as_str() {
                                "mysql" => format!("ALTER TABLE {} DROP PRIMARY KEY;", table_name),
                                _ => format!(
                                    "ALTER TABLE {} DROP CONSTRAINT {}_pkey;",
                                    table_name, table_name
                                ),
                            };
                            diff_statements.push(drop_pk);
                        }
                        if !target_table.primary_key.is_empty() {
                            diff_statements.push(format!(
                                "ALTER TABLE {} ADD PRIMARY KEY ({});",
                                table_name,
                                target_table.primary_key.join(", ")
                            ));
                        }
                    }
                }

                // C. Foreign Key Diffs
                for live_fk in &live_table.foreign_keys {
                    let exists_in_target = target_table.foreign_keys.iter().any(|tfk| {
                        tfk.columns == live_fk.columns
                            && tfk.foreign_table == live_fk.foreign_table
                            && tfk.referred_columns == live_fk.referred_columns
                    });

                    if !exists_in_target {
                        if db_type == "sqlite" {
                            diff_statements.push(format!(
                                    "-- WARNING: Foreign key removal on '{}' ({:?}) requires table rebuild in SQLite.",
                                    table_name, live_fk.columns
                                ));
                        } else {
                            let fk_name = live_fk.name.clone().unwrap_or_else(|| {
                                format!("fk_{}_{}", table_name, live_fk.columns.join("_"))
                            });
                            let drop_fk = match db_type.as_str() {
                                "mysql" => format!(
                                    "ALTER TABLE {} DROP FOREIGN KEY {};",
                                    table_name, fk_name
                                ),
                                _ => format!(
                                    "ALTER TABLE {} DROP CONSTRAINT {};",
                                    table_name, fk_name
                                ),
                            };
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
                        if db_type == "sqlite" {
                            diff_statements.push(format!(
                                    "-- WARNING: Adding foreign key on '{}' ({:?} -> {}) requires table rebuild in SQLite.",
                                    table_name, target_fk.columns, target_fk.foreign_table
                                ));
                        } else {
                            let fk_name = target_fk.name.clone().unwrap_or_else(|| {
                                format!("fk_{}_{}", table_name, target_fk.columns.join("_"))
                            });
                            diff_statements.push(format!(
                                    "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({});",
                                    table_name,
                                    fk_name,
                                    target_fk.columns.join(", "),
                                    target_fk.foreign_table,
                                    target_fk.referred_columns.join(", ")
                                ));
                        }
                    }
                }

                // D. Secondary Index Diffs
                for live_idx in &live_table.indexes {
                    let exists_in_target = target_table.indexes.iter().any(|tidx| {
                        tidx.columns == live_idx.columns && tidx.unique == live_idx.unique
                    });

                    if !exists_in_target {
                        // In MySQL, do not drop an index if it is required by a foreign key constraint
                        if db_type == "mysql"
                            && (target_table
                                .foreign_keys
                                .iter()
                                .any(|fk| fk.columns == live_idx.columns)
                                || live_table
                                    .foreign_keys
                                    .iter()
                                    .any(|fk| fk.columns == live_idx.columns))
                        {
                            continue;
                        }

                        let drop_idx = match db_type.as_str() {
                            "mysql" => format!("DROP INDEX {} ON {};", live_idx.name, table_name),
                            _ => format!("DROP INDEX {};", live_idx.name),
                        };
                        diff_statements.push(drop_idx);
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

    // Check for Dropped Tables (Must be dropped in reverse topological order)
    let sorted_live_tables = topological_sort(&live_schema.tables);
    for table_name in sorted_live_tables.iter().rev() {
        if !target_schema.tables.contains_key(table_name) {
            diff_statements.push(format!("DROP TABLE {};", table_name));
        }
    }

    // Check for Dropped Tables
    for table_name in live_schema.tables.keys() {
        if !target_schema.tables.contains_key(table_name) {
            diff_statements.push(format!("DROP TABLE {};", table_name));
        }
    }

    // --- FUNCTIONS ---
    for (signature, target_func) in &target_schema.functions {
        match live_schema.functions.get(signature) {
            None => {
                diff_statements.push(format!("{};", target_func.definition));
            }
            Some(live_func) => {
                // Compare normalized strings to avoid whitespace mismatch
                let live_clean = live_func
                    .definition
                    .replace(|c: char| c.is_whitespace(), "");
                let target_clean = target_func
                    .definition
                    .replace(|c: char| c.is_whitespace(), "");

                let is_same = if db_type == "mysql" && !live_clean.is_empty() {
                    target_clean.contains(&live_clean)
                } else {
                    live_clean == target_clean
                };

                if !is_same {
                    if db_type == "mysql" {
                        diff_statements
                            .push(format!("DROP FUNCTION IF EXISTS {};", target_func.name));
                        diff_statements.push(format!(
                            "{};",
                            target_func.definition.trim().trim_end_matches(';')
                        ));
                    } else {
                        // For PostgreSQL, use CREATE OR REPLACE
                        diff_statements.push(format!(
                            "{};",
                            target_func
                                .definition
                                .replace("CREATE FUNCTION", "CREATE OR REPLACE FUNCTION")
                        ));
                    }
                }
            }
        }
    }

    // Check for Dropped Functions
    for signature in live_schema.functions.keys() {
        if !target_schema.functions.contains_key(signature) {
            // Drop by signature (includes args for Postgres compatibility)
            diff_statements.push(format!("DROP FUNCTION {};", signature));
        }
    }

    // --- TRIGGERS ---
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

                let is_same = if db_type == "mysql" && !live_clean.is_empty() {
                    target_clean.contains(&live_clean)
                } else {
                    live_clean == target_clean
                };

                if !is_same {
                    let drop_trig = match db_type.as_str() {
                        "sqlite" => format!("DROP TRIGGER {};", target_trigger.name),
                        "mysql" => format!("DROP TRIGGER IF EXISTS {};", target_trigger.name),
                        _ => format!(
                            "DROP TRIGGER {} ON {};",
                            target_trigger.name, target_trigger.table
                        ),
                    };
                    diff_statements.push(drop_trig);
                    diff_statements.push(format!(
                        "{};",
                        target_trigger.definition.trim().trim_end_matches(';')
                    ));
                }
            }
        }
    }

    // Check for Dropped Triggers
    for (key, trigger) in &live_schema.triggers {
        if !target_schema.triggers.contains_key(key) {
            let drop_trig = match db_type.as_str() {
                "sqlite" => format!("DROP TRIGGER {};", trigger.name),
                "mysql" => format!("DROP TRIGGER IF EXISTS {};", trigger.name),
                _ => format!("DROP TRIGGER {} ON {};", trigger.name, trigger.table),
            };
            diff_statements.push(drop_trig);
        }
    }

    // 4. Output Migration File & Update Lockfile
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

fn should_include_table(table_name: &str, config: &TernConfig) -> bool {
    if table_name.eq_ignore_ascii_case("_tern_migrations") {
        return false;
    }
    if config
        .filter
        .exclude_tables
        .iter()
        .any(|t| t.eq_ignore_ascii_case(table_name))
    {
        return false;
    }
    if !config.filter.include_tables.is_empty() {
        return config
            .filter
            .include_tables
            .iter()
            .any(|t| t.eq_ignore_ascii_case(table_name));
    }
    true
}

fn clean_ident(s: &str) -> String {
    s.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
        .to_lowercase()
}

fn extract_table_name(obj: &ObjectName) -> String {
    obj.0
        .last()
        .map(|i| clean_ident(&i.value))
        .unwrap_or_default()
}

fn normalize_type(dtype: &str) -> String {
    let mut lower = dtype.to_lowercase();

    // Strip all whitespace to normalize differences like "VARCHAR (50)" vs "varchar(50)"
    lower.retain(|c| !c.is_whitespace());

    // 1. Normalize sea-schema's Debug format for bounded strings
    if lower.starts_with("string(some(") {
        let len = lower
            .trim_start_matches("string(some(")
            .trim_end_matches("))");
        return format!("varchar({})", len);
    }

    // 2. Normalize sea-schema's unbounded string type
    if lower == "string(none)" {
        return "text".to_string();
    }

    // 3. Normalize sqlparser's Postgres character varying alias
    if lower.starts_with("charactervarying(") {
        let len = lower
            .trim_start_matches("charactervarying(")
            .trim_end_matches(')');
        return format!("varchar({})", len);
    }

    // 4. Group all integer variants (ignoring MySQL display widths like int(11))
    if lower.starts_with("int") || lower == "integer" || lower == "serial" {
        return "int".to_string();
    }

    // 5. Group timestamps
    if lower.contains("timestamp") || lower.contains("datetime") {
        return "timestamp".to_string();
    }

    // 6. Group UUIDs
    if lower.contains("uuid") {
        return "uuid".to_string();
    }

    lower
}

async fn introspect_live_db(
    db_type: &str,
    db_url: &str,
    config: &TernConfig,
) -> Result<NormalizedSchema, Box<dyn std::error::Error>> {
    let mut schema = NormalizedSchema::default();

    match db_type {
        "postgres" => {
            let pool = PgPool::connect(db_url).await?;
            let live_schema = PgDiscovery::new(pool.clone(), "public").discover().await?;

            // Query pg_indexes for secondary indexes & unique constraints
            let idx_rows = sqlx::query(
                "SELECT tablename, indexname, indexdef FROM pg_indexes WHERE schemaname = 'public'",
            )
            .fetch_all(&pool)
            .await?;

            for t in live_schema.tables {
                let t_name = t.info.name.to_lowercase();
                if !should_include_table(&t_name, config) {
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
                        if let (Some(start), Some(end)) = (idx_def.rfind('('), idx_def.rfind(')')) {
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

            // 2. Fetch Custom ENUMs
            let enum_rows = sqlx::query(
                "SELECT t.typname AS name, array_agg(e.enumlabel ORDER BY e.enumsortorder) AS variants
                 FROM pg_type t
                 JOIN pg_enum e ON t.oid = e.enumtypid
                 JOIN pg_namespace n ON n.oid = t.typnamespace
                 WHERE n.nspname = 'public'
                 GROUP BY t.typname",
            )
            .fetch_all(&pool)
            .await?;

            for row in enum_rows {
                let name: String = row.get("name");
                let variants: Vec<String> = row.get("variants");
                schema
                    .enums
                    .insert(name.clone(), NormalizedEnum { name, variants });
            }

            // 3. Fetch Functions
            let func_rows = sqlx::query(
                "SELECT p.proname AS name,
                        pg_get_function_identity_arguments(p.oid) AS args,
                        pg_get_functiondef(p.oid) AS def
                 FROM pg_proc p
                 JOIN pg_namespace n ON p.pronamespace = n.oid
                 WHERE n.nspname = 'public'",
            )
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

            // 4. Fetch Triggers
            let trig_rows = sqlx::query(
                "SELECT t.tgname AS name, c.relname AS table_name, pg_get_triggerdef(t.oid) AS def
                 FROM pg_trigger t
                 JOIN pg_class c ON t.tgrelid = c.oid
                 JOIN pg_namespace n ON c.relnamespace = n.oid
                 WHERE n.nspname = 'public' AND NOT t.tgisinternal",
            )
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
        "mysql" => {
            let pool = MySqlPool::connect(db_url).await?;
            let live_schema = MySqlDiscovery::new(pool.clone(), "tern_db")
                .discover()
                .await?;
            for t in live_schema.tables {
                let t_name = t.info.name.to_lowercase();
                if !should_include_table(&t_name, config) {
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

            // 1. Fetch Functions (Routines)
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

            // 2. Fetch Triggers
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
        }
        "sqlite" => {
            let pool = SqlitePool::connect(db_url).await?;
            let live_schema = SqliteDiscovery::new(pool.clone()).discover().await?;
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

                // Query SQLite PRAGMA foreign_key_list
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

                // Query SQLite PRAGMA index_list & index_info
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

            // 1. Fetch Triggers
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
        }
        _ => return Err("Unsupported database type".into()),
    }

    Ok(schema)
}

fn extract_enums_from_sql(sql: &str) -> (BTreeMap<String, NormalizedEnum>, String) {
    let mut enums = BTreeMap::new();
    let mut remaining = String::new();
    let chars: Vec<char> = sql.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let rest = &sql[i..];
        let lower = rest.to_lowercase();
        let trimmed_lower = lower.trim_start();
        if trimmed_lower.starts_with("create")
            && (trimmed_lower.starts_with("create type")
                || trimmed_lower[6..].trim_start().starts_with("type"))
        {
            if let Some(semi_pos) = rest.find(';') {
                let stmt_str = &rest[..semi_pos];
                let stmt_lower = stmt_str.to_lowercase();
                if stmt_lower.contains("as enum") {
                    if let Some(as_enum_idx) = stmt_lower.find("as enum") {
                        let before_as = stmt_str[..as_enum_idx].trim();
                        let type_name = before_as
                            .split_whitespace()
                            .last()
                            .map(clean_ident)
                            .unwrap_or_default();

                        if let (Some(lparen), Some(rparen)) =
                            (stmt_str.find('('), stmt_str.rfind(')'))
                        {
                            let inside = &stmt_str[lparen + 1..rparen];
                            let variants: Vec<String> = inside
                                .split(',')
                                .map(|s| s.trim().trim_matches('\'').trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();

                            if !type_name.is_empty() {
                                enums.insert(
                                    type_name.clone(),
                                    NormalizedEnum {
                                        name: type_name,
                                        variants,
                                    },
                                );
                            }
                        }
                    }
                    i += semi_pos + 1;
                    continue;
                }
            }
        }
        remaining.push(chars[i]);
        i += 1;
    }

    (enums, remaining)
}

fn extract_triggers_from_sql(sql: &str) -> (BTreeMap<String, NormalizedTrigger>, String) {
    let mut triggers = BTreeMap::new();
    let mut remaining = String::new();
    let chars: Vec<char> = sql.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let rest = &sql[i..];
        let lower = rest.to_lowercase();
        let trimmed_lower = lower.trim_start();
        if trimmed_lower.starts_with("create")
            && (trimmed_lower.contains("trigger")
                && (trimmed_lower.starts_with("create trigger")
                    || trimmed_lower.starts_with("create or replace trigger")
                    || trimmed_lower.starts_with("create temporary trigger")
                    || trimmed_lower.starts_with("create constraint trigger")))
        {
            let mut end_pos = None;
            let mut in_begin = 0;
            let mut j = 0;
            let rest_chars: Vec<char> = rest.chars().collect();
            while j < rest_chars.len() {
                let sub = &rest[j..].to_lowercase();
                if sub.starts_with("begin")
                    && !sub[5..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
                {
                    in_begin += 1;
                    j += 5;
                    continue;
                }
                if sub.starts_with("end")
                    && !sub[3..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
                {
                    if in_begin > 0 {
                        in_begin -= 1;
                    }
                    j += 3;
                    continue;
                }
                if rest_chars[j] == ';' && in_begin == 0 {
                    end_pos = Some(j);
                    break;
                }
                j += 1;
            }

            if let Some(semi_pos) = end_pos {
                let stmt_str = rest[..=semi_pos].trim();
                let stmt_lower = stmt_str.to_lowercase();

                // Find trigger name right after TRIGGER keyword
                if let Some(trig_idx) = stmt_lower.find("trigger") {
                    let after_trigger = stmt_str[trig_idx + 7..].trim_start();
                    let mut tokens = after_trigger.split_whitespace();
                    let mut first = tokens.next().unwrap_or_default();
                    if first.eq_ignore_ascii_case("if") {
                        let _not = tokens.next();
                        let _exists = tokens.next();
                        first = tokens.next().unwrap_or_default();
                    }
                    let trig_name = clean_ident(first);

                    // Find table name right after ON keyword
                    if let Some(on_idx) = stmt_lower.find(" on ") {
                        let after_on = stmt_str[on_idx + 4..].trim_start();
                        let tbl_name = after_on
                            .split_whitespace()
                            .next()
                            .map(clean_ident)
                            .unwrap_or_default();

                        if !trig_name.is_empty() && !tbl_name.is_empty() {
                            let key = format!("{}_{}", tbl_name, trig_name);
                            triggers.insert(
                                key,
                                NormalizedTrigger {
                                    name: trig_name,
                                    table: tbl_name,
                                    definition: stmt_str.trim().trim_end_matches(';').to_string(),
                                },
                            );
                        }
                    }
                }
                i += semi_pos + 1;
                continue;
            }
        }
        remaining.push(chars[i]);
        i += 1;
    }

    (triggers, remaining)
}

fn extract_functions_from_sql(
    db_type: &str,
    sql: &str,
) -> (BTreeMap<String, NormalizedFunction>, String) {
    let mut functions = BTreeMap::new();
    let mut remaining = String::new();
    let chars: Vec<char> = sql.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let rest = &sql[i..];
        let lower = rest.to_lowercase();
        let trimmed_lower = lower.trim_start();
        if trimmed_lower.starts_with("create")
            && (trimmed_lower.starts_with("create function")
                || trimmed_lower.starts_with("create or replace function")
                || trimmed_lower.starts_with("create definite function"))
        {
            let mut end_pos = None;
            let mut in_dollar_quote: Option<String> = None;
            let mut in_begin = 0;
            let mut j = 0;
            let rest_chars: Vec<char> = rest.chars().collect();
            while j < rest_chars.len() {
                if in_dollar_quote.is_none() {
                    if rest_chars[j] == '$' {
                        if let Some(end_dollar) = rest[j + 1..].find('$') {
                            let tag = &rest[j..=j + 1 + end_dollar];
                            if tag
                                .chars()
                                .skip(1)
                                .take(tag.len() - 2)
                                .all(|ch| ch.is_alphanumeric() || ch == '_')
                            {
                                in_dollar_quote = Some(tag.to_string());
                                j += tag.len();
                                continue;
                            }
                        }
                    }
                    let sub = &rest[j..].to_lowercase();
                    if sub.starts_with("begin")
                        && !sub[5..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
                    {
                        in_begin += 1;
                        j += 5;
                        continue;
                    }
                    if sub.starts_with("end")
                        && !sub[3..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
                    {
                        if in_begin > 0 {
                            in_begin -= 1;
                        }
                        j += 3;
                        continue;
                    }
                    if rest_chars[j] == ';' && in_begin == 0 {
                        end_pos = Some(j);
                        break;
                    }
                } else if let Some(ref tag) = in_dollar_quote {
                    if rest[j..].starts_with(tag) {
                        j += tag.len();
                        in_dollar_quote = None;
                        continue;
                    }
                }
                j += 1;
            }

            if let Some(semi_pos) = end_pos {
                let stmt_str = rest[..=semi_pos].trim();
                let stmt_lower = stmt_str.to_lowercase();

                if let Some(func_idx) = stmt_lower.find("function") {
                    let after_func = stmt_str[func_idx + 8..].trim_start();
                    let func_name = after_func
                        .split(|c: char| c == '(' || c.is_whitespace())
                        .next()
                        .map(clean_ident)
                        .unwrap_or_default();

                    let mut arg_types = Vec::new();
                    if let (Some(lparen), Some(rparen)) = (stmt_str.find('('), stmt_str.find(')')) {
                        if rparen > lparen {
                            let args_str = &stmt_str[lparen + 1..rparen];
                            for arg in args_str.split(',') {
                                let arg_trimmed = arg.trim();
                                if !arg_trimmed.is_empty() {
                                    if let Some(dt) = arg_trimmed.split_whitespace().last() {
                                        arg_types.push(clean_ident(dt));
                                    }
                                }
                            }
                        }
                    }

                    let signature = if db_type == "postgres" {
                        format!("{}({})", func_name.to_lowercase(), arg_types.join(", "))
                    } else {
                        func_name.clone()
                    };

                    if !func_name.is_empty() {
                        functions.insert(
                            signature.clone(),
                            NormalizedFunction {
                                name: func_name,
                                signature,
                                definition: stmt_str.trim().trim_end_matches(';').to_string(),
                            },
                        );
                    }
                }

                i += semi_pos + 1;
                continue;
            }
        }
        remaining.push(chars[i]);
        i += 1;
    }

    (functions, remaining)
}

fn parse_target_schema(
    db_type: &str,
    schema_path: &str,
    config: &TernConfig,
) -> Result<NormalizedSchema, Box<dyn std::error::Error>> {
    let path = Path::new(schema_path);
    if !path.exists() {
        return Err(format!("Schema path '{}' does not exist.", schema_path).into());
    }

    let mut sql_files: Vec<PathBuf> = Vec::new();
    if path.is_file() {
        sql_files.push(path.to_path_buf());
    } else {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) == Some("sql") {
                sql_files.push(p);
            }
        }
        sql_files.sort();
    }

    let mut combined_sql = String::new();
    for file in sql_files {
        combined_sql.push_str(&fs::read_to_string(file)?);
        combined_sql.push('\n');
    }

    let (enums, without_enums) = extract_enums_from_sql(&combined_sql);
    let (triggers, without_triggers) = extract_triggers_from_sql(&without_enums);
    let (functions, remaining_sql) = extract_functions_from_sql(db_type, &without_triggers);

    let mut schema = NormalizedSchema {
        tables: BTreeMap::new(),
        enums,
        triggers,
        functions,
    };

    let dialect: Box<dyn sqlparser::dialect::Dialect> = match db_type {
        "mysql" => Box::new(MySqlDialect {}),
        "sqlite" => Box::new(SQLiteDialect {}),
        _ => Box::new(PostgreSqlDialect {}),
    };

    let statements = Parser::parse_sql(&*dialect, &remaining_sql)?;
    let mut standalone_indexes: Vec<(String, NormalizedIndex)> = Vec::new();

    for stmt in statements {
        match stmt {
            Statement::CreateTable(ref create_table) => {
                let t_name = extract_table_name(&create_table.name);
                if !should_include_table(&t_name, config) {
                    continue;
                }

                let mut cols = BTreeMap::new();
                let mut pk_cols = Vec::new();
                let mut fks = Vec::new();
                let mut indexes = Vec::new();

                // 1. Inspect Inline Column Options
                for col in &create_table.columns {
                    let c_name = clean_ident(&col.name.value);
                    let mut not_null = false;

                    for opt_def in &col.options {
                        match &opt_def.option {
                            ColumnOption::NotNull => not_null = true,
                            ColumnOption::Unique { is_primary, .. } => {
                                if *is_primary {
                                    not_null = true;
                                    pk_cols.push(c_name.clone());
                                } else {
                                    let idx_name = opt_def
                                        .name
                                        .as_ref()
                                        .map(|i| clean_ident(&i.value))
                                        .unwrap_or_else(|| {
                                            format!("idx_{}_{}_unique", t_name, c_name)
                                        });
                                    indexes.push(NormalizedIndex {
                                        name: idx_name,
                                        columns: vec![c_name.clone()],
                                        unique: true,
                                    });
                                }
                            }
                            ColumnOption::ForeignKey {
                                foreign_table,
                                referred_columns,
                                ..
                            } => {
                                let fk_name = opt_def.name.as_ref().map(|i| clean_ident(&i.value));
                                let ref_table = extract_table_name(foreign_table);
                                let ref_cols = if referred_columns.is_empty() {
                                    vec!["id".to_string()]
                                } else {
                                    referred_columns
                                        .iter()
                                        .map(|i| clean_ident(&i.value))
                                        .collect()
                                };
                                fks.push(NormalizedForeignKey {
                                    name: fk_name,
                                    columns: vec![c_name.clone()],
                                    foreign_table: ref_table,
                                    referred_columns: ref_cols,
                                });
                            }
                            _ => {}
                        }
                    }

                    cols.insert(
                        c_name.clone(),
                        NormalizedColumn {
                            name: c_name,
                            data_type: col.data_type.to_string(),
                            not_null,
                        },
                    );
                }

                // 2. Inspect Table-Level Constraints
                for constraint in &create_table.constraints {
                    match constraint {
                        TableConstraint::PrimaryKey { columns, .. } => {
                            for col in columns {
                                let c_name = clean_ident(&col.value);
                                if !pk_cols.contains(&c_name) {
                                    pk_cols.push(c_name);
                                }
                            }
                        }
                        TableConstraint::Unique { name, columns, .. } => {
                            let u_cols: Vec<String> =
                                columns.iter().map(|c| clean_ident(&c.value)).collect();
                            let idx_name = name
                                .as_ref()
                                .map(|i| clean_ident(&i.value))
                                .unwrap_or_else(|| {
                                    format!("idx_{}_{}_unique", t_name, u_cols.join("_"))
                                });
                            indexes.push(NormalizedIndex {
                                name: idx_name,
                                columns: u_cols,
                                unique: true,
                            });
                        }
                        TableConstraint::ForeignKey {
                            name,
                            columns,
                            foreign_table,
                            referred_columns,
                            ..
                        } => {
                            fks.push(NormalizedForeignKey {
                                name: name.as_ref().map(|i| clean_ident(&i.value)),
                                columns: columns.iter().map(|c| clean_ident(&c.value)).collect(),
                                foreign_table: extract_table_name(foreign_table),
                                referred_columns: referred_columns
                                    .iter()
                                    .map(|c| clean_ident(&c.value))
                                    .collect(),
                            });
                        }
                        _ => {}
                    }
                }

                schema.tables.insert(
                    t_name.clone(),
                    NormalizedTable {
                        name: t_name,
                        columns: cols,
                        primary_key: pk_cols,
                        foreign_keys: fks,
                        indexes,
                        raw_create_sql: Some(stmt.to_string()),
                    },
                );
            }
            Statement::CreateIndex(create_idx) => {
                let t_name = extract_table_name(&create_idx.table_name);
                if !should_include_table(&t_name, config) {
                    continue;
                }
                let idx_cols: Vec<String> = create_idx
                    .columns
                    .iter()
                    .map(|c| clean_ident(&c.expr.to_string()))
                    .collect();
                let idx_name = create_idx
                    .name
                    .as_ref()
                    .map(extract_table_name)
                    .unwrap_or_else(|| format!("idx_{}_{}", t_name, idx_cols.join("_")));

                standalone_indexes.push((
                    t_name,
                    NormalizedIndex {
                        name: idx_name,
                        columns: idx_cols,
                        unique: create_idx.unique,
                    },
                ));
            }
            Statement::CreateFunction {
                ref name, ref args, ..
            } => {
                let f_name = extract_table_name(name);
                let definition = stmt.to_string();

                let arg_types: Vec<String> = args
                    .as_ref()
                    .map(|v| v.iter().map(|a| a.data_type.to_string()).collect())
                    .unwrap_or_default();

                let signature = if db_type == "postgres" {
                    format!("{}({})", f_name, arg_types.join(", "))
                } else {
                    f_name.clone()
                };

                schema.functions.insert(
                    signature.clone(),
                    NormalizedFunction {
                        name: f_name,
                        signature,
                        definition,
                    },
                );
            }
            _ => {}
        }
    }

    // Attach standalone CREATE INDEX statements to their target tables
    for (t_name, idx) in standalone_indexes {
        if let Some(table) = schema.tables.get_mut(&t_name) {
            table.indexes.push(idx);
        }
    }

    Ok(schema)
}

// Add to the bottom of src/diff.rs
fn topological_sort(tables: &BTreeMap<String, NormalizedTable>) -> Vec<String> {
    let mut sorted = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut visiting = std::collections::HashSet::new();

    fn visit(
        table: &str,
        tables: &BTreeMap<String, NormalizedTable>,
        visited: &mut std::collections::HashSet<String>,
        visiting: &mut std::collections::HashSet<String>,
        sorted: &mut Vec<String>,
    ) {
        if visited.contains(table) {
            return;
        }
        // Break circular references to avoid infinite loops
        if visiting.contains(table) {
            return;
        }

        visiting.insert(table.to_string());

        if let Some(t) = tables.get(table) {
            for fk in &t.foreign_keys {
                visit(&fk.foreign_table, tables, visited, visiting, sorted);
            }
        }

        visiting.remove(table);
        visited.insert(table.to_string());
        sorted.push(table.to_string());
    }

    for table in tables.keys() {
        visit(table, tables, &mut visited, &mut visiting, &mut sorted);
    }

    sorted
}
