use super::model::{
    clean_ident, extract_table_name, should_include_table, NormalizedColumn, NormalizedEnum,
    NormalizedForeignKey, NormalizedFunction, NormalizedIndex, NormalizedSchema, NormalizedTable,
    NormalizedTrigger,
};
use crate::config::TernConfig;
use sqlparser::ast::{ColumnOption, Statement, TableConstraint};
use sqlparser::dialect::{MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub fn extract_enums_from_sql(sql: &str) -> (BTreeMap<String, NormalizedEnum>, String) {
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

pub fn extract_triggers_from_sql(sql: &str) -> (BTreeMap<String, NormalizedTrigger>, String) {
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

pub fn extract_functions_from_sql(
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

pub fn parse_target_schema(
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
