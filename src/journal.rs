//! The event log: one line per button press, with the readings beside it.

use std::fs::{create_dir_all, read_to_string, OpenOptions};
use std::io::Write as _;

use crate::paths;

/// Appends one stamped line. A failure here must not take the handler down, so
/// it is reported and swallowed.
pub fn append(line: &str) {
    let path = paths::log();
    if let Some(dir) = path.parent() {
        let _ = create_dir_all(dir);
    }

    let stamped = format!(
        "{}  {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        line
    );

    let written = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(stamped.as_bytes()));

    if let Err(e) = written {
        eprintln!("cannot write {}: {e}", path.display());
    }
}

/// The tail of the log, with the raw dumps left out so the status screen stays
/// readable.
pub fn recent(count: usize) -> Vec<String> {
    let Ok(text) = read_to_string(paths::log()) else {
        return vec!["no log yet".to_string()];
    };

    let lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("0x22"))
        .rev()
        .take(count)
        .collect();

    if lines.is_empty() {
        return vec!["nothing logged yet".to_string()];
    }
    lines.into_iter().rev().map(str::to_string).collect()
}
