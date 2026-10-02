//! The button-to-command map in `actions.txt`.

use std::collections::HashMap;
use std::fs::{create_dir_all, read_to_string, write};
use std::process::Command;

use anyhow::{Context, Result};

use crate::paths;

const TEMPLATE: &str = "\
# One line per button: <code> = <command line>
#
# The code is the one the log shows for that button. The command runs through
# cmd, so pipes and arguments work. This file is read again on every press, so
# changes take effect without restarting the handler.
#
# Mind that the handler runs elevated, and whatever it starts inherits that.
#
# Two keys are free, in the sense that the hardware does nothing with them:
#
#   0x22006F  the key marked P1 (Fn+F4), which is what MSI reserved for this
#   0x220029  the key next to the power button
#
# 0x22006F = wt.exe
";

/// Reads the map, or an empty one if the file is missing.
pub fn load() -> HashMap<u32, String> {
    read_to_string(paths::actions())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Unparsable lines are skipped rather than fatal: a typo in a config file
/// should not stop the buttons working.
fn parse(text: &str) -> HashMap<u32, String> {
    let mut actions = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((code, command)) = line.split_once('=') else {
            continue;
        };
        let command = command.trim();
        if command.is_empty() {
            continue;
        }
        if let Some(code) = parse_code(code.trim()) {
            actions.insert(code, command.to_string());
        }
    }

    actions
}

/// Hex with the prefix the log prints, or plain decimal.
fn parse_code(text: &str) -> Option<u32> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Writes the commented example the first time, so there is something to edit.
pub fn write_template() -> Result<()> {
    let path = paths::actions();
    if path.exists() {
        return Ok(());
    }
    create_dir_all(paths::dir())?;
    write(&path, TEMPLATE)?;
    Ok(())
}

/// Starts a mapped command without waiting for it.
pub fn run(command: &str) -> Result<()> {
    Command::new("cmd")
        .args(["/C", command])
        .spawn()
        .with_context(|| format!("could not start: {command}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_radixes() {
        assert_eq!(parse_code("0x220029"), Some(0x22_0029));
        assert_eq!(parse_code("0X220029"), Some(0x22_0029));
        assert_eq!(parse_code("2228265"), Some(0x22_0029));
        assert_eq!(parse_code("not a code"), None);
    }

    #[test]
    fn keeps_the_whole_command_line() {
        let actions = parse("0x220029 = cmd /c echo hello = world");
        assert_eq!(
            actions.get(&0x22_0029).map(String::as_str),
            Some("cmd /c echo hello = world")
        );
    }

    #[test]
    fn skips_comments_blanks_and_nonsense() {
        let actions = parse(
            "\n# 0x220004 = never\n\n   \nnot a line\n0x220032 =\nbogus = wt.exe\n0x220004 = wt.exe\n",
        );
        assert_eq!(actions.len(), 1);
        assert!(actions.contains_key(&0x22_0004));
    }

    #[test]
    fn the_template_parses_to_nothing() {
        assert!(parse(TEMPLATE).is_empty());
    }
}
