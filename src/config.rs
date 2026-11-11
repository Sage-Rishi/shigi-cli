use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "tern.toml";
pub const LOCK_FILE: &str = "tern.lock";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TernConfig {
    pub project: ProjectConfig,
    pub paths: PathConfig,
    pub filter: FilterConfig,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProjectConfig {
    pub db_type: String,
    pub engine: String,
    pub target_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PathConfig {
    pub schema_path: String,
    pub migrations_dir: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FilterConfig {
    pub include_tables: Vec<String>,
    pub exclude_tables: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct MigrationLock {
    pub total_hash: String,
    // BTreeMap keeps migration filenames sorted chronologically
    pub files: BTreeMap<String, String>,
}

impl TernConfig {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        if !Path::new(CONFIG_FILE).exists() {
            return Err("tern.toml not found. Please run `tern init` first.".into());
        }
        let content = fs::read_to_string(CONFIG_FILE)?;
        Ok(toml::from_str(&content)?)
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        fs::write(CONFIG_FILE, content)?;
        Ok(())
    }
}

/// Computes SHA-256 for each .sql file and a chained total hash
pub fn compute_migrations_lock(
    migrations_dir: &str,
) -> Result<MigrationLock, Box<dyn std::error::Error>> {
    let dir_path = Path::new(migrations_dir);
    let mut files_map = BTreeMap::new();

    if dir_path.exists() {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir_path)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sql"))
            .collect();
        entries.sort();

        for path in entries {
            let filename = path.file_name().unwrap().to_string_lossy().to_string();
            let content = fs::read(&path)?;
            let mut hasher = Sha256::new();
            hasher.update(&content);
            let hash = hex::encode(hasher.finalize());
            files_map.insert(filename, hash);
        }
    }

    let mut total_hasher = Sha256::new();
    for (name, hash) in &files_map {
        total_hasher.update(name.as_bytes());
        total_hasher.update(hash.as_bytes());
    }
    let total_hash = hex::encode(total_hasher.finalize());

    Ok(MigrationLock {
        total_hash,
        files: files_map,
    })
}

/// Verifies that existing migration files haven't been tampered with
pub fn verify_lockfile(migrations_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let lock_path = Path::new(migrations_dir).join(LOCK_FILE);
    if !lock_path.exists() {
        return Ok(());
    }

    let lock_content = fs::read_to_string(&lock_path)?;
    let existing_lock: MigrationLock = toml::from_str(&lock_content)?;
    let current_lock = compute_migrations_lock(migrations_dir)?;

    for (file, old_hash) in &existing_lock.files {
        match current_lock.files.get(file) {
            Some(curr_hash) if curr_hash != old_hash => {
                return Err(format!(
                    "Integrity Error: Migration '{}' was modified! Expected {}, got {}",
                    file, old_hash, curr_hash
                )
                .into());
            }
            None => {
                return Err(format!(
                    "Integrity Error: Previously tracked migration '{}' is missing!",
                    file
                )
                .into());
            }
            _ => {}
        }
    }

    if existing_lock.files.len() == current_lock.files.len()
        && existing_lock.total_hash != current_lock.total_hash
    {
        return Err("Total migration hash mismatch! Lockfile is out of sync.".into());
    }

    println!(
        "-> Migration hashes verified (Total Hash: {}...)",
        &current_lock.total_hash[..12]
    );
    Ok(())
}

pub fn write_lockfile(migrations_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let lock = compute_migrations_lock(migrations_dir)?;
    let lock_path = Path::new(migrations_dir).join(LOCK_FILE);
    let content = toml::to_string_pretty(&lock)?;
    fs::write(lock_path, content)?;
    Ok(())
}
