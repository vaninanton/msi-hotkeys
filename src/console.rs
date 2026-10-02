//! The `--status` screen: a reader, so it can be left open beside the running
//! handler.

use std::io::Write as _;
use std::time::Duration;

use anyhow::Result;

use crate::ec::Machine;
use crate::model::Identity;
use crate::{journal, readings, win};

const INTERVAL: Duration = Duration::from_secs(1);

/// Redraws once a second: the labelled readings, the fan curve the controller is
/// following, every EC byte raw, and the last few button presses out of the log.
///
/// Never returns; the user leaves with Ctrl+C.
pub fn status(machine: &Machine, identity: &Identity) -> Result<()> {
    // Windows consoles do not interpret escape sequences until asked to, and one
    // that has not been asked prints them as text instead of acting on them. If
    // the request fails the screen scrolls rather than redrawing, which is worth
    // having anyway.
    let vt = win::enable_virtual_terminal();

    loop {
        if vt {
            print!("\u{1B}[2J\u{1B}[H");
        } else {
            println!("\n{}", "-".repeat(78));
        }

        let ec = machine.read();

        println!(
            "msi-hotkeys     {}     Ctrl+C to leave",
            chrono::Local::now().format("%H:%M:%S")
        );
        println!();
        println!("  machine: {}", identity.describe());
        println!("  {}", readings::summary(&ec, machine.power().as_ref()));
        if let Some(firmware) = ec.firmware() {
            println!("  EC firmware: {firmware}");
        }
        println!(
            "  buttons: {}",
            match ec.software_has_buttons() {
                Some(true) => "handled by this program",
                Some(false) => "left to the firmware — no events are raised",
                None => "unreadable",
            }
        );

        println!();
        println!("  fan curve, as the controller has it:");
        for (label, class) in [("CPU", "MSI_CPU"), ("GPU", "MSI_VGA")] {
            let points: Vec<String> = ec
                .curve(class)
                .iter()
                .map(|(temperature, value)| format!("{temperature}C:{value}"))
                .collect();
            println!("    {label}  {}", points.join("  "));
        }

        println!();
        println!("  EC windows:");
        for (class, values) in ec.windows() {
            match values {
                Some(values) => {
                    let bytes: Vec<String> =
                        values.iter().map(|value| format!("{value:02X}")).collect();
                    println!("    {class:20} {}", bytes.join(" "));
                }
                None => println!("    {class:20} <unreadable>"),
            }
        }

        println!();
        println!("  recent presses:");
        for line in journal::recent(6) {
            println!("    {line}");
        }

        std::io::stdout().flush().ok();
        std::thread::sleep(INTERVAL);
    }
}
