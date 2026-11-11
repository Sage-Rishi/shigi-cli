// src/lib.rs
pub mod cli;
pub mod config;
pub mod db;
pub mod differential_engine;
pub mod engine;
pub mod verify;

// Backward-compatible alias for existing callers
pub use differential_engine as diff;
