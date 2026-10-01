//! The button-to-command map, kept in a text file the user can also edit by
//! hand. Read on every press, so an edit takes effect without a restart.

use std::collections::BTreeMap;
use std::fs::{create_dir_all, read_to_string, write};
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};

const TEMPLATE: &str = "\
# One line per button: <code> = <command line>
#
# The code is the one the log shows for that button. The command runs through
# cmd, so pipes and arguments work. This file is read again on every press, so
# changes take effect without restarting the handler.
#
# Mind that the handler runs elevated, and whatever it starts inherits that.
#
# 0x220029 = wt.exe
";

pub fn config_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("msi-hotkeys")
}

pub fn actions_path() -> PathBuf {
    config_dir().join("actions.txt")
}

pub fn log_path() -> PathBuf {
    config_dir().join("events.log")
}

/// Reads the map. Unparsable lines are skipped rather than fatal: a typo in a
/// config file should not stop the buttons working.
pub fn load() -> BTreeMap<u32, String> {
    let mut actions = BTreeMap::new();

    let Ok(text) = read_to_string(actions_path()) else {
        return actions;
    };

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((code, command)) = line.split_once('=') else {
            continue;
        };
        if let Some(code) = parse_code(code.trim()) {
            actions.insert(code, command.trim().to_string());
        }
    }

    actions
}

/// Rewrites the file from the map, keeping the explanatory header so the file
/// stays hand-editable.
pub fn save(actions: &BTreeMap<u32, String>) -> Result<()> {
    create_dir_all(config_dir())?;

    let mut text = String::from(TEMPLATE);
    text.push('\n');
    for (code, command) in actions {
        text.push_str(&format!("0x{code:06X} = {command}\n"));
    }

    write(actions_path(), text).context("could not write actions.txt")
}

pub fn write_template_if_missing() -> Result<()> {
    let path = actions_path();
    if path.exists() {
        return Ok(());
    }
    create_dir_all(config_dir())?;
    write(&path, TEMPLATE)?;
    Ok(())
}

fn parse_code(text: &str) -> Option<u32> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Starts a mapped command without waiting for it.
pub fn run(command: &str) -> Result<()> {
    Command::new("cmd")
        .args(["/C", command])
        .spawn()
        .with_context(|| format!("could not start: {command}"))?;
    Ok(())
}

/// Opens a file with whatever Windows associates with it.
pub fn open_in_shell(path: &PathBuf) -> Result<()> {
    // The empty argument is the window title that `start` would otherwise take
    // the path for.
    Command::new("cmd")
        .args(["/C", "start", "", &path.to_string_lossy()])
        .spawn()?;
    Ok(())
}
