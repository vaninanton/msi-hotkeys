//! Looks for the buttons on the vendor HID collections, as an alternative to
//! the ACPI-WMI channel.
//!
//! The WMI channel needs arming by something in MSI's software before it emits
//! anything (measured: after a boot in which the service never ran, a
//! subscription receives nothing). HID is a different path entirely: if the
//! keyboard controller reports the buttons as HID input, no arming and no
//! elevation are needed, and the handler becomes self-contained.
//!
//! The machine exposes three collections under VID_3151&PID_3020:
//! COL02 system control, COL03 consumer control, COL04 vendor-defined. Windows
//! may hold the first two open exclusively; the vendor one is the hope.

use std::time::{Duration, Instant};

use anyhow::Result;
use hidapi::HidApi;

const VENDOR_ID: u16 = 0x3151;
const LISTEN_SECONDS: u64 = 45;

fn main() -> Result<()> {
    let api = HidApi::new()?;

    println!("--- HID interfaces of vendor {VENDOR_ID:04X} ---");
    let mut paths = Vec::new();
    for info in api.device_list() {
        if info.vendor_id() != VENDOR_ID {
            continue;
        }
        println!(
            "  usage {:04X}:{:04X}  iface {}  {:?}",
            info.usage_page(),
            info.usage(),
            info.interface_number(),
            info.path()
        );
        paths.push(info.path().to_owned());
    }

    if paths.is_empty() {
        println!("nothing found — is the vendor id right?");
        return Ok(());
    }

    // Open what we can. A collection Windows keeps for itself fails here, which
    // is information too, so the error is printed rather than fatal.
    println!("\n--- opening ---");
    let mut open = Vec::new();
    for path in &paths {
        match api.open_path(path) {
            Ok(device) => {
                device.set_blocking_mode(false)?;
                println!("  opened {path:?}");
                open.push((path.to_owned(), device));
            }
            Err(e) => println!("  REFUSED {path:?}: {e}"),
        }
    }

    if open.is_empty() {
        println!("\nevery collection is held by the system; HID is not a way in from here.");
        return Ok(());
    }

    println!("\n--- press the buttons; listening {LISTEN_SECONDS} s ---");
    let deadline = Instant::now() + Duration::from_secs(LISTEN_SECONDS);
    let mut buffer = [0u8; 64];
    let mut total = 0usize;

    while Instant::now() < deadline {
        for (path, device) in &open {
            match device.read(&mut buffer) {
                Ok(0) => {}
                Ok(n) => {
                    total += 1;
                    let hex: Vec<String> =
                        buffer[..n].iter().map(|b| format!("{b:02X}")).collect();
                    // The tail of the path is enough to tell the collections apart.
                    let tag = path.to_string_lossy();
                    let tag = tag.rsplit('&').next().unwrap_or("?").to_string();
                    println!("  [{tag}] {n} bytes: {}", hex.join(" "));
                }
                // A collection Windows reads exclusively refuses every ReadFile, so
            // report it once and stop polling it rather than spamming the log.
            Err(e) => {
                println!("  read error on {path:?}: {e}");
                dead.push(path.clone());
            }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    println!("\n{total} report(s) seen.");
    if total == 0 {
        println!("The buttons do not come through these collections.");
    }
    Ok(())
}
