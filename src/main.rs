// A windowed program: without this, Windows gives every run a console window.
// The reading modes borrow the launching terminal instead, see
// `win::attach_parent_console`.
#![windows_subsystem = "windows"]

//! Handler for the hardware buttons of an MSI GL72 6QD (board MS-1796).
//!
//! It replaces MSI's software completely: no SCM.exe, no `Micro Star SCM`
//! service, no ring-0 driver. It does need administrator rights — the ACPI-WMI
//! classes refuse reads without them, and the refusal arrives on the first read
//! rather than when the namespace is opened, so startup tests it by reading.
//!
//! Three jobs. It arms the event channel, which is the whole trick of this
//! project — see [`ec::Machine::set_handover`]. It subscribes to the ACPI-WMI event
//! the buttons raise (`root\WMI` : `MSI_Event`, `MSIEvt` : `UInt32`). And it runs
//! whatever the user has mapped to a button in `actions.txt`.
//!
//! It deliberately does not touch the fan: Cooler Boost is carried out by the
//! controller itself, and the event is only a notification.
//!
//! The one byte it writes is the handover flag, and the map that byte comes from
//! was decoded from one machine's ACPI tables — hence the hardware gate in
//! [`model`].

mod actions;
mod buttons;
mod console;
mod ec;
mod handler;
mod journal;
mod model;
mod paths;
mod readings;
mod tray;
mod win;

use anyhow::{bail, Result};

use ec::Machine;
use model::Identity;

const USAGE: &str = "\
msi-hotkeys — handler for the MSI hardware buttons (board MS-1796)

  msi-hotkeys              sit in the notification area (default)
  msi-hotkeys --console    run in the foreground, logging to the terminal
  msi-hotkeys --status     read-only screen, safe to run beside the handler
  msi-hotkeys --force      skip the hardware check (read docs/EC-MAP.md first)
  msi-hotkeys --help       this text

Needs administrator rights: the ACPI-WMI classes refuse reads without them.
";

enum Mode {
    Tray,
    Console,
    Status,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let given = |name: &str| args.iter().any(|arg| arg == name);

    let mode = if given("--status") {
        Mode::Status
    } else if given("--console") {
        Mode::Console
    } else {
        Mode::Tray
    };

    // Everything that prints wants the terminal it was started from; tray mode
    // wants no console at all, which is what the crate attribute arranges.
    if !matches!(mode, Mode::Tray) || given("--help") {
        win::attach_parent_console();
    }

    if given("--help") {
        print!("{USAGE}");
        return Ok(());
    }

    let identity = Identity::read()?;
    if !identity.is_supported() && !given("--force") {
        let message = model::refusal(&identity);
        eprintln!("{message}");
        win::notify("msi-hotkeys", &message);
        bail!("unsupported machine: {}", identity.describe());
    }

    // Tested by reading rather than by connecting: opening the namespace
    // succeeds without elevation and the refusal only arrives on the first read
    // of an ACPI class.
    let machine = Machine::open()?;
    if !machine.can_read() {
        let message = "Классы ACPI-WMI не читаются: нужны права администратора";
        eprintln!("{message}");
        win::notify("msi-hotkeys", message);
        bail!(message);
    }

    // A reader: it arms nothing and subscribes to nothing, so it may run beside
    // the handler and needs no claim on the name below.
    if matches!(mode, Mode::Status) {
        return console::status(&machine, &identity);
    }

    // Two handlers would both claim the buttons and both run whatever is mapped to
    // a pressed button, so the second start bows out. Claimed only after the
    // checks above: a copy that cannot work must not sit there holding the name,
    // or a stray double-click would block the working handler.
    if !win::claim_single_instance() {
        println!("already running");
        win::notify("msi-hotkeys", "Обработчик уже запущен");
        return Ok(());
    }

    match mode {
        Mode::Console => {
            win::release_buttons_on_console_exit();
            handler::listen()
        }
        Mode::Tray => tray::run(),
        Mode::Status => unreachable!("handled above"),
    }
}
