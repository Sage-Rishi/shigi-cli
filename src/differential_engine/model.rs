use crate::config::TernConfig;
use sqlparser::ast::ObjectName;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedColumn {
    pub name: String,
    pub data_type: String,
    pub not_null: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NormalizedForeignKey {
    pub name: Option<String>,
    pub columns: Vec<String>,
    pub foreign_table: String,
    pub referred_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NormalizedIndex {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone)]
pub struct NormalizedTable {
    pub name: String,
    pub columns: BTreeMap<String, NormalizedColumn>,
    pub primary_key: Vec<String>,
    pub foreign_keys: Vec<NormalizedForeignKey>,
    pub indexes: Vec<NormalizedIndex>,
    pub raw_create_sql: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedEnum {
    pub name: String,
    pub variants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedTrigger {
    pub name: String,
    pub table: String,
    pub definition: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedFunction {
    pub name: String,
    pub signature: String, // e.g., "add(integer, integer)"
    pub definition: String,
}

#[derive(Debug, Clone, Default)]
pub struct NormalizedSchema {
    pub tables: BTreeMap<String, NormalizedTable>,
    pub enums: BTreeMap<String, NormalizedEnum>,
    pub triggers: BTreeMap<String, NormalizedTrigger>,
    pub functions: BTreeMap<String, NormalizedFunction>,
}

pub fn should_include_schema(schema_name: &str, config: &TernConfig) -> bool {
    let clean = clean_ident(schema_name);
    if config
        .filter
        .exclude_schemas
        .iter()
        .any(|s| clean_ident(s).eq_ignore_ascii_case(&clean))
    {
        return false;
    }
    if !config.filter.include_schemas.is_empty() {
        return config
            .filter
            .include_schemas
            .iter()
            .any(|s| clean_ident(s).eq_ignore_ascii_case(&clean));
    }
    true
}

pub fn should_include_bare_table(table_name: &str, config: &TernConfig) -> bool {
    let clean = clean_ident(table_name);
    if clean.eq_ignore_ascii_case("_tern_migrations") {
        return false;
    }
    if config
        .filter
        .exclude_tables
        .iter()
        .any(|t| clean_ident(t).eq_ignore_ascii_case(&clean))
    {
        return false;
    }
    if !config.filter.include_tables.is_empty() {
        return config
            .filter
            .include_tables
            .iter()
            .any(|t| clean_ident(t).eq_ignore_ascii_case(&clean));
    }
    true
}

pub fn should_include_table(table_name: &str, config: &TernConfig) -> bool {
    let clean = clean_ident(table_name);
    if let Some((schema, tbl)) = clean.split_once('.') {
        if !should_include_schema(schema, config) {
            return false;
        }
        return should_include_bare_table(tbl, config);
    }
    should_include_bare_table(&clean, config)
}

pub fn should_include_table_with_schema(
    schema_name: Option<&str>,
    table_name: &str,
    config: &TernConfig,
) -> bool {
    if let Some(schema) = schema_name {
        if !should_include_schema(schema, config) {
            return false;
        }
    }
    should_include_table(table_name, config)
}

pub fn clean_ident(s: &str) -> String {
    s.trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
        .to_lowercase()
}

pub fn extract_table_name(obj: &ObjectName) -> String {
    obj.0
        .last()
        .map(|i| clean_ident(&i.value))
        .unwrap_or_default()
}

pub fn extract_schema_name(obj: &ObjectName) -> Option<String> {
    if obj.0.len() > 1 {
        Some(clean_ident(&obj.0[0].value))
    } else {
        None
    }
}

pub fn normalize_type(dtype: &str) -> String {
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
