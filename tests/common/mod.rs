// tests/common/mod.rs
#![allow(dead_code)]
use bollard::Docker;
use std::fs;
use std::path::{Path, PathBuf};
use tern::config::{FilterConfig, PathConfig, ProjectConfig, TernConfig};
use tern::db;
use tern::differential_engine;
use tern::engine;
use tokio::sync::Mutex;

/// Global mutex to serialize test execution so container-based and shadow DB tests
/// run strictly one-by-one, avoiding port 5432/3306 conflicts and container name collisions.
pub static CONTAINER_TEST_LOCK: Mutex<()> = Mutex::const_new(());

/// Safely checks if a container daemon (Docker/Podman) is running and accessible
pub async fn detect_working_container_engine() -> Option<String> {
    // 1. Try Docker via macOS socket or standard /var/run/docker.sock
    if let Ok(home) = std::env::var("HOME") {
        let mac_sock = format!("{}/.docker/run/docker.sock", home);
        if Path::new(&mac_sock).exists() {
            if let Ok(docker) =
                Docker::connect_with_unix(&mac_sock, 2, bollard::API_DEFAULT_VERSION)
            {
                if docker.ping().await.is_ok() {
                    return Some("docker".to_string());
                }
            }
        }
    }
    if Path::new("/var/run/docker.sock").exists() {
        if let Ok(docker) = Docker::connect_with_socket_defaults() {
            if docker.ping().await.is_ok() {
                return Some("docker".to_string());
            }
        }
    }

    // 2. Try Podman via macOS machine socket or standard runtime dir
    if let Ok(home) = std::env::var("HOME") {
        let podman_sock = format!(
            "{}/.local/share/containers/podman/machine/podman-machine-default/api.sock",
            home
        );
        if Path::new(&podman_sock).exists() {
            if let Ok(docker) =
                Docker::connect_with_unix(&podman_sock, 2, bollard::API_DEFAULT_VERSION)
            {
                if docker.ping().await.is_ok() {
                    return Some("podman".to_string());
                }
            }
        }
    }
    if let Ok(xrd) = std::env::var("XDG_RUNTIME_DIR") {
        let podman_sock = format!("{}/podman/podman.sock", xrd);
        if Path::new(&podman_sock).exists() {
            if let Ok(docker) =
                Docker::connect_with_unix(&podman_sock, 2, bollard::API_DEFAULT_VERSION)
            {
                if docker.ping().await.is_ok() {
                    return Some("podman".to_string());
                }
            }
        }
    }

    None
}

/// Helper to read the most recently generated migration file in a directory
pub fn get_latest_migration_content(dir: &str) -> String {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    entries.sort();

    let latest = entries.last().expect("No migration file found");
    fs::read_to_string(latest).unwrap()
}

/// Simulates the `tern generate` workflow for a given schema and migrations directory
pub async fn simulate_tern_generate(
    db_type: &str,
    engine: &str,
    schema_sql: &str,
    migrations_dir: &str,
    schema_path: &str,
    step_name: &str,
) {
    let _guard = CONTAINER_TEST_LOCK.lock().await;

    fs::write(schema_path, schema_sql).unwrap();

    let config = TernConfig {
        project: ProjectConfig {
            db_type: db_type.to_string(),
            engine: engine.to_string(),
            target_url: None,
        },
        paths: PathConfig {
            schema_path: schema_path.to_string(),
            migrations_dir: migrations_dir.to_string(),
        },
        filter: FilterConfig {
            include_tables: vec![],
            exclude_tables: vec!["_tern_migrations".to_string()],
        },
    };

    if db_type != "sqlite" {
        engine::provision_container(engine, db_type).await.unwrap();
    }

    let (pool, db_url) = db::connect(db_type).await.unwrap();
    db::apply_migrations(&pool, migrations_dir).await.unwrap();
    differential_engine::generate_diff(&config, &db_url, step_name)
        .await
        .unwrap();

    if db_type == "sqlite" {
        let path = db_url.strip_prefix("sqlite://").unwrap_or("tern_shadow.db");
        let _ = fs::remove_file(path);
    } else {
        engine::teardown_container(engine).await.unwrap();
    }
}
