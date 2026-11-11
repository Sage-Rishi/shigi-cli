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

pub async fn generate_diff(
    config: &TernConfig,
    db_url: &str,
    migration_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let db_type = config.project.db_type.to_lowercase();

    // 1. Introspect Live Database State
    let live_tables = introspect_live_db(&db_type, db_url, config).await?;
    println!(
        "-> [Live State] Found {} filtered table(s).",
        live_tables.len()
    );

    // 2. Parse Target Declarative Schema
    let target_tables = parse_target_schema(&db_type, &config.paths.schema_path, config)?;
    println!(
        "-> [Target State] Found {} filtered table(s).",
        target_tables.len()
    );

    // 3. Compute SQL Diff Statements
    let mut diff_statements: Vec<String> = Vec::new();

    for (table_name, target_table) in &target_tables {
        match live_tables.get(table_name) {
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
                // --- A. Column Diffs ---
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

                // --- B. Primary Key Diffs ---
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

                // --- C. Foreign Key Diffs (Compared by structural signature) ---
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

                // --- D. Secondary Index Diffs ---
                for live_idx in &live_table.indexes {
                    let exists_in_target = target_table.indexes.iter().any(|tidx| {
                        tidx.columns == live_idx.columns && tidx.unique == live_idx.unique
                    });

                    if !exists_in_target {
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

    // Check for Dropped Tables
    for table_name in live_tables.keys() {
        if !target_tables.contains_key(table_name) {
            diff_statements.push(format!("DROP TABLE {};", table_name));
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
) -> Result<BTreeMap<String, NormalizedTable>, Box<dyn std::error::Error>> {
    let mut tables = BTreeMap::new();

    match db_type {
        "postgres" => {
            let pool = PgPool::connect(db_url).await?;
            let schema = PgDiscovery::new(pool.clone(), "public").discover().await?;

            // Query pg_indexes for secondary indexes & unique constraints
            let idx_rows = sqlx::query(
                "SELECT tablename, indexname, indexdef FROM pg_indexes WHERE schemaname = 'public'",
            )
            .fetch_all(&pool)
            .await?;

            for t in schema.tables {
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

                            // Avoid duplicating single-column inline UNIQUE constraints if identical
                            indexes.push(NormalizedIndex {
                                name: idx_name.to_lowercase(),
                                columns: idx_cols,
                                unique,
                            });
                        }
                    }
                }

                tables.insert(
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
        }
        "mysql" => {
            let pool = MySqlPool::connect(db_url).await?;
            let schema = MySqlDiscovery::new(pool, "tern_db").discover().await?;
            for t in schema.tables {
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

                for idx in t.indexes {
                    let idx_name = idx.name.to_lowercase();
                    let idx_cols: Vec<String> =
                        idx.parts.iter().map(|p| p.column.to_lowercase()).collect();

                    if idx_name == "primary" {
                        pk_cols = idx_cols;
                    } else if !fk_names.contains(&idx_name) {
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

                tables.insert(
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
        }
        "sqlite" => {
            let pool = SqlitePool::connect(db_url).await?;
            let schema = SqliteDiscovery::new(pool.clone()).discover().await?;
            for t in schema.tables {
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

                tables.insert(
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
        }
        _ => return Err("Unsupported database type".into()),
    }

    Ok(tables)
}

fn parse_target_schema(
    db_type: &str,
    schema_path: &str,
    config: &TernConfig,
) -> Result<BTreeMap<String, NormalizedTable>, Box<dyn std::error::Error>> {
    let path = Path::new(schema_path);
    if !path.exists() {
        return Err(format!("Schema path '{}' does not exist.", schema_path).into());
    }

    let mut sql_files: Vec<PathBuf> = Vec::new();
    if path.is_file() {
        sql_files.push(path.to_path_buf());
    } else {
        for entry in fs::read_dir(path)? {
            let p = entry?.path();
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

    let statements = match db_type {
        "postgres" => Parser::parse_sql(&PostgreSqlDialect {}, &combined_sql)?,
        "mysql" => Parser::parse_sql(&MySqlDialect {}, &combined_sql)?,
        "sqlite" => Parser::parse_sql(&SQLiteDialect {}, &combined_sql)?,
        _ => return Err("Unsupported database type".into()),
    };

    let mut tables = BTreeMap::new();
    let mut standalone_indexes: Vec<(String, NormalizedIndex)> = Vec::new();

    for stmt in statements {
        match &stmt {
            Statement::CreateTable(create_table) => {
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

                tables.insert(
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
            _ => {}
        }
    }

    // Attach standalone CREATE INDEX statements to their target tables
    for (t_name, idx) in standalone_indexes {
        if let Some(table) = tables.get_mut(&t_name) {
            table.indexes.push(idx);
        }
    }

    Ok(tables)
}
