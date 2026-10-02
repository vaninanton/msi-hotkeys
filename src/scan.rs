//! `--scan`: what the keyboard actually sends, for the keys that raise no
//! ACPI-WMI event.
//!
//! Fn+F2 and Fn+F7 do something — the first opens Windows' projection dialog —
//! yet neither reaches `MSI_Event`. So they travel as ordinary input, and this
//! mode reads that input directly.
//!
//! Raw Input is the right tool and the only one that works here. Reading the
//! HID collections through `ReadFile` was tried and refused: Windows holds the
//! keyboard collections exclusively (see docs/FINDINGS.md). Raw Input is the
//! supported way to see input that another driver owns, and `RIDEV_INPUTSINK`
//! delivers it even though this process has no focused window.
//!
//! Deliberately bounded. It runs for the number of seconds asked for and exits;
//! it is not wired into the tray and nothing logs keystrokes in normal use. The
//! difference matters: a program that quietly records everything typed is a
//! keylogger, whatever it was meant for.

use std::fmt::Write as _;
use std::fs::{create_dir_all, write};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::win::{KeyEvent, RawInput};
use crate::{paths, win};

/// How long a scan runs when no duration is given.
pub const DEFAULT_SECONDS: u64 = 30;

/// Collects raw input for the given time, printing as it goes and leaving the
/// whole run in `keys.log`.
pub fn run(seconds: u64) -> Result<()> {
    let mut session = RawInput::open()?;
    let deadline = Instant::now() + Duration::from_secs(seconds);

    println!("scanning raw input for {seconds} s — press the keys now");
    println!(
        "nothing is recorded after that; the run is saved to {}",
        paths::keys().display()
    );
    println!();

    win::notify(
        "msi-hotkeys",
        &format!("Сканирую клавиатуру {seconds} с — нажимайте клавиши"),
    );

    let mut report = String::new();
    let mut seen = 0_usize;

    while Instant::now() < deadline {
        for event in session.poll() {
            seen += 1;
            let line = describe(&event);
            println!("{line}");
            let _ = writeln!(report, "{line}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let path = paths::keys();
    if let Some(dir) = path.parent() {
        let _ = create_dir_all(dir);
    }
    write(&path, &report)?;

    println!();
    println!("{seen} events; saved to {}", path.display());
    win::notify(
        "msi-hotkeys",
        &format!("Сканирование закончено: {seen} событий"),
    );

    Ok(())
}

/// One event as a line. Scan codes are what matters here, so they lead.
fn describe(event: &KeyEvent) -> String {
    match event {
        KeyEvent::Keyboard {
            make_code,
            flags,
            vkey,
            message,
        } => {
            // Bit 0 of the flags is the break — key up rather than key down.
            let direction = if flags & 1 == 0 { "down" } else { "up  " };
            format!(
                "keyboard  scan 0x{make_code:02X}  {direction}  vkey 0x{vkey:02X}  \
                 flags 0x{flags:02X}  msg 0x{message:04X}"
            )
        }
        KeyEvent::Hid { device, bytes } => {
            let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
            format!("hid       {}  [{}]", short_device(device), hex.join(" "))
        }
    }
}

/// Device paths are long and mostly boilerplate; the useful part is the vendor
/// and product plus the collection number.
fn short_device(device: &str) -> String {
    device
        .split('#')
        .nth(1)
        .map_or_else(|| device.to_string(), str::to_string)
}
