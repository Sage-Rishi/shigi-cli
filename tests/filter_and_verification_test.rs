// tests/filter_and_verification_test.rs
use std::fs;
use tern::config::{
    verify_lockfile, write_lockfile, FilterConfig, PathConfig, ProjectConfig, TernConfig,
};
use tern::differential_engine::model::{
    should_include_bare_table, should_include_schema, should_include_table,
    should_include_table_with_schema,
};
use tern::differential_engine::parser::parse_target_schema;
use tern::verify::run_verify;

#[test]
fn test_schema_filtering_logic() {
    let mut config = TernConfig {
        project: ProjectConfig {
            db_type: "postgres".to_string(),
            engine: "docker".to_string(),
            target_url: None,
        },
        paths: PathConfig {
            schema_path: "schema".to_string(),
            migrations_dir: "migrations".to_string(),
        },
        filter: FilterConfig {
            include_tables: vec!["users".to_string(), "orders".to_string()],
            exclude_tables: vec!["_tern_migrations".to_string(), "secret_data".to_string()],
            include_schemas: vec!["public".to_string(), "app".to_string()],
            exclude_schemas: vec!["internal".to_string(), "audit".to_string()],
        },
    };

    // Test should_include_schema
    assert!(should_include_schema("public", &config));
    assert!(should_include_schema("app", &config));
    assert!(!should_include_schema("internal", &config));
    assert!(!should_include_schema("audit", &config));
    // Not in include_schemas:
    assert!(!should_include_schema("analytics", &config));

    // When include_schemas is empty, any non-excluded schema is included
    config.filter.include_schemas.clear();
    assert!(should_include_schema("analytics", &config));
    assert!(!should_include_schema("internal", &config));
    assert!(!should_include_schema("audit", &config));

    // Test table and schema combinations
    assert!(should_include_table_with_schema(
        Some("public"),
        "users",
        &config
    ));
    assert!(!should_include_table_with_schema(
        Some("internal"),
        "users",
        &config
    ));
    assert!(!should_include_table_with_schema(
        Some("public"),
        "secret_data",
        &config
    ));
    assert!(!should_include_table_with_schema(
        Some("public"),
        "_tern_migrations",
        &config
    ));

    // Qualified table names
    assert!(should_include_table("public.users", &config));
    assert!(!should_include_table("internal.users", &config));
    assert!(!should_include_table("public.secret_data", &config));
    assert!(!should_include_bare_table("secret_data", &config));
}

#[test]
fn test_schema_filtering_in_ddl_parser() {
    let test_dir = "tests/temp_schema_filter_parser";
    let _ = fs::remove_dir_all(test_dir);
    fs::create_dir_all(test_dir).unwrap();

    let schema_file = format!("{}/schema.sql", test_dir);
    let ddl = r#"
        CREATE TABLE app.users (
            id INT PRIMARY KEY,
            username VARCHAR(50) NOT NULL
        );

        CREATE TABLE audit.logs (
            id INT PRIMARY KEY,
            message TEXT NOT NULL
        );

        CREATE TABLE public.products (
            id INT PRIMARY KEY,
            title VARCHAR(100) NOT NULL
        );
    "#;
    fs::write(&schema_file, ddl).unwrap();

    // 1. Exclude the "audit" schema
    let config = TernConfig {
        project: ProjectConfig {
            db_type: "postgres".to_string(),
            engine: "none".to_string(),
            target_url: None,
        },
        paths: PathConfig {
            schema_path: schema_file.clone(),
            migrations_dir: format!("{}/migrations", test_dir),
        },
        filter: FilterConfig {
            include_tables: vec![],
            exclude_tables: vec![],
            include_schemas: vec![],
            exclude_schemas: vec!["audit".to_string()],
        },
    };

    let schema = parse_target_schema("postgres", &schema_file, &config).unwrap();
    assert!(schema.tables.contains_key("users"));
    assert!(schema.tables.contains_key("products"));
    assert!(
        !schema.tables.contains_key("logs"),
        "Table from excluded schema 'audit' must not be present"
    );

    // 2. Whitelist only "app" schema
    let mut config_whitelist = config.clone();
    config_whitelist.filter.exclude_schemas.clear();
    config_whitelist.filter.include_schemas = vec!["app".to_string()];

    let schema_whitelisted =
        parse_target_schema("postgres", &schema_file, &config_whitelist).unwrap();
    assert!(schema_whitelisted.tables.contains_key("users"));
    assert!(
        !schema_whitelisted.tables.contains_key("products"),
        "Table in 'public' should be excluded when only 'app' is whitelisted"
    );
    assert!(
        !schema_whitelisted.tables.contains_key("logs"),
        "Table in 'audit' should be excluded when only 'app' is whitelisted"
    );

    let _ = fs::remove_dir_all(test_dir);
}

#[tokio::test]
async fn test_verify_command_and_lockfile_integrity() {
    let test_dir = "tests/temp_verify_ci_test";
    let _ = fs::remove_dir_all(test_dir);
    let migrations_dir = format!("{}/migrations", test_dir);
    let schema_dir = format!("{}/schema", test_dir);
    fs::create_dir_all(&migrations_dir).unwrap();
    fs::create_dir_all(&schema_dir).unwrap();

    let schema_file = format!("{}/schema.sql", schema_dir);
    fs::write(
        &schema_file,
        "CREATE TABLE accounts (id INT PRIMARY KEY, balance INT NOT NULL);",
    )
    .unwrap();

    let config_path = format!("{}/tern.toml", test_dir);
    let config = TernConfig {
        project: ProjectConfig {
            db_type: "sqlite".to_string(),
            engine: "none".to_string(),
            target_url: None,
        },
        paths: PathConfig {
            schema_path: schema_file.clone(),
            migrations_dir: migrations_dir.clone(),
        },
        filter: FilterConfig {
            include_tables: vec![],
            exclude_tables: vec!["_tern_migrations".to_string()],
            include_schemas: vec![],
            exclude_schemas: vec![],
        },
    };
    fs::write(&config_path, toml::to_string_pretty(&config).unwrap()).unwrap();

    // 1. Initial empty state: verify should succeed
    let res = run_verify(None, Some(config_path.clone())).await;
    assert!(res.is_ok(), "Clean project should verify successfully");

    // 2. Create migration file but DO NOT create lockfile: verify must fail!
    let mig1 = format!("{}/20261111000000_init.sql", migrations_dir);
    fs::write(&mig1, "CREATE TABLE accounts (id INT PRIMARY KEY);").unwrap();

    let missing_lock_res = run_verify(None, Some(config_path.clone())).await;
    assert!(
        missing_lock_res.is_err(),
        "Verification must fail when migrations exist without a lockfile"
    );
    let err_str = missing_lock_res.unwrap_err().to_string();
    assert!(err_str.contains("tern.lock' is missing"));

    // 3. Generate lockfile: verify should now succeed
    write_lockfile(&migrations_dir).unwrap();
    let verified_res = run_verify(None, Some(config_path.clone())).await;
    assert!(
        verified_res.is_ok(),
        "Verification should succeed with valid lockfile: {:?}",
        verified_res
    );

    // 4. Modify migration file content: checksum mismatch must fail!
    fs::write(
        &mig1,
        "CREATE TABLE accounts (id INT PRIMARY KEY, balance INT);",
    )
    .unwrap();
    let modified_res = run_verify(None, Some(config_path.clone())).await;
    assert!(
        modified_res.is_err(),
        "Verification must fail when migration is modified"
    );
    let err_str = modified_res.unwrap_err().to_string();
    assert!(err_str.contains("was modified! Expected SHA-256"));

    // Revert migration content so it matches again
    fs::write(&mig1, "CREATE TABLE accounts (id INT PRIMARY KEY);").unwrap();
    assert!(verify_lockfile(&migrations_dir).is_ok());

    // 5. Add an untracked new migration file: verify must fail!
    let untracked = format!("{}/20261111000001_untracked.sql", migrations_dir);
    fs::write(&untracked, "CREATE TABLE extra (id INT);").unwrap();
    let untracked_res = run_verify(None, Some(config_path.clone())).await;
    assert!(
        untracked_res.is_err(),
        "Verification must fail when untracked migration file is added"
    );
    let err_str = untracked_res.unwrap_err().to_string();
    assert!(err_str.contains("Untracked migration file"));

    // Remove untracked file
    fs::remove_file(&untracked).unwrap();
    assert!(verify_lockfile(&migrations_dir).is_ok());

    // 6. Delete tracked migration file: verify must fail!
    fs::remove_file(&mig1).unwrap();
    let deleted_res = run_verify(None, Some(config_path.clone())).await;
    assert!(
        deleted_res.is_err(),
        "Verification must fail when tracked migration file is deleted"
    );
    let err_str = deleted_res.unwrap_err().to_string();
    assert!(
        err_str.contains("tracks 1 migration(s)") || err_str.contains("is missing from"),
        "Unexpected error: {}",
        err_str
    );

    // 7. Schema syntax error: verify must fail!
    fs::write(&schema_file, "INVALID SQL SYNTAX AT LINE 1").unwrap();
    let bad_schema_res = run_verify(None, Some(config_path.clone())).await;
    assert!(
        bad_schema_res.is_err(),
        "Verification must fail when target schema has syntax errors"
    );

    let _ = fs::remove_dir_all(test_dir);
}
