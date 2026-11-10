use clap::{Parser, Subcommand};
use inquire::Select;
use std::path::Path;

#[derive(Parser)]
#[command(
    name = "tern",
    about = "Spins up a DB container and generates schema migrations"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Skip the prompt and force a specific container engine (docker or podman)
    #[arg(short, long, global = true)]
    pub engine: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Generate migrations based on schema diff
    Generate {
        /// Target database type (postgres or mysql)
        #[arg(short, long, default_value = "postgres")]
        db_type: String,
    },
}

pub fn prompt_for_engine() -> String {
    let mut available_engines = Vec::new();

    if Path::new("/var/run/docker.sock").exists()
        || Path::new("/Users/bitsage/.docker/run/docker.sock").exists()
    {
        available_engines.push("Docker");
    }

    if Path::new(
        "/Users/bitsage/.local/share/containers/podman/machine/podman-machine-default/api.sock",
    )
    .exists()
    {
        available_engines.push("Podman");
    }

    if available_engines.is_empty() {
        eprintln!("No running container daemons found. Please start Docker or Podman.");
        std::process::exit(1);
    }

    if available_engines.len() == 1 {
        let auto_selected = available_engines[0];
        println!("Detected {} running. Auto-selecting...", auto_selected);
        return auto_selected.to_string();
    }

    Select::new(
        "Multiple container engines detected. Which one would you like to use?",
        available_engines,
    )
    .prompt()
    .unwrap_or_else(|_| {
        eprintln!("Selection cancelled.");
        std::process::exit(1);
    })
    .to_string()
}
