//! Where the program keeps its two files.

use std::path::PathBuf;

/// `%LOCALAPPDATA%\msi-hotkeys`, falling back to the temp directory on the
/// off chance the variable is missing: losing the log is better than failing to
/// start.
pub fn dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
    base.join("msi-hotkeys")
}

pub fn actions() -> PathBuf {
    dir().join("actions.txt")
}

pub fn log() -> PathBuf {
    dir().join("events.log")
}
