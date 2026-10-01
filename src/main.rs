// A windowed program: without this, Windows gives every run a console window.
// The `--console` and `--status` modes borrow the launching terminal instead,
// see `attach_parent_console`.
#![windows_subsystem = "windows"]

//! Handler for the hardware buttons of an MSI GL72 6QD.
//!
//! It replaces MSI's software completely: no SCM.exe, no `Micro Star SCM`
//! service, no ring-0 driver, and no elevation — measured 2026-10-02, an
//! ordinary session reads root\WMI, writes the arming byte and receives the
//! events. Access is still checked at startup rather than assumed.
//!
//! Three jobs. It arms the event channel, which is the whole trick of this
//! project — see `set_arming`. It subscribes to the ACPI-WMI event the buttons
//! raise (root\WMI : MSI_Event, MSIEvt : UInt32). And it runs whatever the user
//! has mapped to a button in `actions.txt`.
//!
//! Started with no arguments it sits in the notification area. `--console` runs
//! it in the foreground instead, and `--status` opens a read-only screen that
//! can be left next to either.
//!
//! It deliberately does not touch the fan: Cooler Boost is carried out by the
//! controller itself, and the event is only a notification.

use std::collections::HashMap;
use std::fs::{create_dir_all, read_to_string, write, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use wmi::{Variant, WMIConnection};

/// Button codes as they arrive in `MSI_Event.MSIEvt`. The low byte matches the
/// scancodes in the Linux driver `drivers/platform/x86/msi-wmi.c`; see
/// KERNEL-REFERENCE.md.
///
/// The codes are not all of the form 0x2200NN: `0x220208`, `0x224957` and
/// `0x224B57` carry something in the middle byte too, so it is not merely a
/// prefix. What it means is not yet known.
const COOLER_BOOST: u32 = 0x22_0004;
const CENTER_KEY: u32 = 0x22_0029; // "MSI M-Center main menu" in the kernel
const VOLUME_DOWN: u32 = 0x22_0021;
const VOLUME_UP: u32 = 0x22_0032;

/// Named from the kernel's own table rather than guessed: the low byte is the
/// scancode, `0x08` is `WIND_KEY_TOUCHPAD` ("Fn+F3 touchpad toggle") and `0x57`
/// is `WIND_KEY_CAMERA` ("Fn+F6 webcam toggle"). The camera turns up under two
/// codes differing only in the middle byte — the pattern this project noticed
/// before knowing what it meant, and most likely on against off.
const TOUCHPAD: u32 = 0x22_0208;
const CAMERA_ONE: u32 = 0x22_4957;
const CAMERA_TWO: u32 = 0x22_4B57;

/// The one EC byte that hands the buttons over to software, and the only thing
/// MSI's service does at startup. Measured on 2026-10-02: it reads 0 after a
/// boot in which no MSI software ran, and 1 once the service has started. While
/// it is 0 the controller deals with the buttons itself and raises no ACPI
/// event at all, so a subscription sits there silent — which is what made this
/// channel look as though it needed the service running.
const ARMING_CLASS: &str = "MSI_Software";
const ARMING_PROPERTY: &str = "Software";
const ARMING_INSTANCE: &str = r"ACPI\PNP0C14\0_0";

/// Where the tachometer sits when Cooler Boost is on. Measured plateau was
/// 137..138 against 77 at idle, so the threshold is well clear of both.
const FAN_AT_MAXIMUM: u64 = 120;

/// The classes that expose single-byte windows into EC RAM, and the property
/// each one carries its value in.
const EC_WINDOWS: [(&str, &str); 7] = [
    ("MSI_AP", "AP"),
    ("MSI_Device", "Device"),
    ("MSI_System", "System"),
    ("MSI_Power", "Power"),
    ("MSI_CPU", "CPU"),
    ("MSI_VGA", "VGA"),
    ("MSI_Master_Battery", "Master_Battery"),
];

/// An MSI_Event indication. The field name differs from the WMI property, so
/// serde is told the wire name; `rename` on the struct makes the `wmi` crate
/// build `SELECT * FROM MSI_Event` for us.
#[derive(Deserialize, Debug)]
#[serde(rename = "MSI_Event")]
struct MsiEvent {
    #[serde(rename = "MSIEvt")]
    code: u32,
}

/// Battery and mains state, straight from the documented WDM provider rather
/// than guessed at in EC RAM.
#[derive(Deserialize, Debug)]
#[serde(rename = "BatteryStatus")]
#[serde(rename_all = "PascalCase")]
struct BatteryStatus {
    power_online: bool,
    charging: bool,
    discharging: bool,
    voltage: u32,
}

#[derive(Deserialize, Debug)]
#[serde(rename = "Win32_Battery")]
#[serde(rename_all = "PascalCase")]
struct Win32Battery {
    estimated_charge_remaining: Option<u16>,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let wanted = |name: &str| args.iter().any(|arg| arg == name);

    // The reading modes want the terminal they were started from; the tray mode
    // wants no console at all, which is what the crate attribute arranges.
    if wanted("--status") || wanted("--console") {
        attach_parent_console();
    }

    if wanted("--status") {
        // A reader: it arms nothing and subscribes to nothing, so it may run
        // beside the handler and needs no claim on the name below.
        return status_screen(&connect()?);
    }

    // Tested before the name is claimed, and deliberately as a capability rather
    // than as a privilege check: what matters is whether root\WMI can be read.
    // A copy started without elevation cannot, and must not sit there holding the
    // name — that would turn a stray double-click into a block on the real one.
    if let Err(e) = connect() {
        eprintln!("{e:#}");
        notify("msi-hotkeys", "Нужны права администратора: root\\WMI не читается");
        return Err(e);
    }

    // Two handlers would both arm the channel and both run whatever is mapped to
    // a pressed button, so the second start bows out.
    if !claim_single_instance() {
        println!("уже запущен");
        notify("msi-hotkeys", "Обработчик уже запущен");
        return Ok(());
    }

    if wanted("--console") {
        return run_in_console();
    }
    run_in_tray()
}

/// Takes the name that marks "a handler is running", reporting whether it was
/// free. The handle is deliberately never closed: the name must stay taken for
/// as long as this process runs, and Windows releases it when the process ends.
///
/// The name is session-local, so it also catches the case worth catching here —
/// a second copy started without elevation, which cannot read root\WMI and would
/// sit there doing nothing. An elevated holder labels the mutex at its own
/// integrity level, so the unelevated attempt is refused outright rather than
/// told the name is taken; either answer means the same thing.
fn claim_single_instance() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    unsafe {
        match CreateMutexW(None, true, w!("Local\\msi-hotkeys-single-instance")) {
            Ok(_) => GetLastError() != ERROR_ALREADY_EXISTS,
            Err(_) => false,
        }
    }
}

/// A windows-subsystem program starts with no console, so the reading modes
/// borrow the one they were launched from and point the standard handles at it.
/// Without re-opening CONOUT$ the handles stay invalid and nothing is printed.
fn attach_parent_console() {
    use windows::core::{s, PCSTR};
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileA, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        AttachConsole, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }

        let open = |name: PCSTR, access: u32| -> Option<HANDLE> {
            CreateFileA(
                name,
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .ok()
        };

        if let Some(out) = open(s!("CONOUT$"), GENERIC_WRITE.0) {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, out);
            let _ = SetStdHandle(STD_ERROR_HANDLE, out);
        }
        if let Some(input) = open(s!("CONIN$"), GENERIC_READ.0) {
            let _ = SetStdHandle(STD_INPUT_HANDLE, input);
        }
    }
}

/// Each connection initialises COM for the thread it is made on, and is
/// `!Send`, so the compiler refuses to let one travel to another thread. That
/// is the whole COM-apartment problem handled at compile time — and the reason
/// the tray and the subscription each make their own.
fn connect() -> Result<WMIConnection> {
    WMIConnection::with_namespace_path(r"root\WMI")
        .context(r"cannot reach root\WMI — the ACPI-WMI classes are not readable here")
}

// --- the handler itself -----------------------------------------------------

/// Arms the channel and then blocks, handling events until the process ends.
fn listen() -> Result<()> {
    let wmi = connect()?;

    match set_arming(&wmi, 1) {
        Ok(true) => println!("armed the channel ({ARMING_CLASS} {ARMING_INSTANCE} -> 1)"),
        Ok(false) => println!("channel was already armed"),
        // Worth continuing: the channel may have been armed earlier in this
        // boot, in which case events arrive anyway.
        Err(e) => eprintln!("WARNING: could not arm the channel: {e:#}"),
    }

    if let Err(e) = write_action_template() {
        eprintln!("could not create the actions file: {e:#}");
    }

    println!("actions: {}", actions_path().display());
    println!("listening for MSI_Event; log: {}", log_path().display());

    // A notification query blocks until an indication arrives, so this loop
    // costs nothing while idle — no polling, no timer.
    for indication in wmi.notification::<MsiEvent>()? {
        match indication {
            Ok(event) => handle(&wmi, event.code),
            Err(e) => eprintln!("subscription error: {e:#}"),
        }
    }

    Ok(())
}

fn run_in_console() -> Result<()> {
    // Ctrl+C would otherwise leave the channel armed, which is survivable but
    // untidy: the point of disarming is to hand the buttons back to firmware.
    install_console_handler();
    listen()
}

/// Writes the arming flag. Returns whether it had to write, so a run on an
/// already-armed machine can say so.
fn set_arming(wmi: &WMIConnection, value: u8) -> Result<bool> {
    // A WMI object path treats the backslash as an escape, so each one in the
    // instance name is doubled.
    let escaped = ARMING_INSTANCE.replace('\u{5C}', "\u{5C}\u{5C}");
    let path = format!("{ARMING_CLASS}.InstanceName=\"{escaped}\"");

    let object = wmi
        .get_object(&path)
        .with_context(|| format!("no such instance: {path}"))?;

    if let Ok(Variant::UI1(current)) = object.get_property(ARMING_PROPERTY) {
        if current == value {
            return Ok(false);
        }
    }

    object
        .put_property(ARMING_PROPERTY, Variant::UI1(value))
        .context("could not set the property on the local copy")?;
    wmi.put_instance(&object)
        .context("the instance update was rejected")?;

    // The EC can refuse silently, so the value is read back rather than trusted.
    let after = wmi.get_object(&path)?.get_property(ARMING_PROPERTY)?;
    if !matches!(after, Variant::UI1(written) if written == value) {
        bail!("the write did not stick, reads back as {after:?}");
    }

    Ok(true)
}

/// Hands the buttons back to the firmware. Called on the way out, from whichever
/// thread is doing the leaving, so it opens a connection of its own.
fn disarm() {
    match connect().and_then(|wmi| set_arming(&wmi, 0)) {
        Ok(true) => println!("disarmed the channel"),
        Ok(false) => println!("channel was not armed"),
        Err(e) => eprintln!("could not disarm: {e:#}"),
    }
}

/// Everything that happens on a button press.
fn handle(wmi: &WMIConnection, code: u32) {
    let button = describe(code);
    println!("0x{code:06X}  {button}");

    // Everything readable goes in the log on every press: the labelled values
    // first, then every EC byte raw. The raw dump is what identifying the
    // remaining buttons and bytes will be done from.
    let status = status_line(wmi);
    log(&format!("0x{code:06X}  {button:14}  {status}"));
    log(&format!("{:24}  {}", "", read_ec(wmi)));

    // The actions file is read per press rather than at startup, so editing it
    // takes effect without restarting the handler.
    if let Some(command) = load_actions().get(&code) {
        match run_action(command) {
            Ok(()) => log(&format!("{:24}  ran: {command}", "")),
            Err(e) => log(&format!("{:24}  FAILED to run {command}: {e:#}", "")),
        }
    } else if code == CENTER_KEY {
        // Nothing mapped yet, so the free button at least reports the machine.
        notify("MSI hotkeys", &status);
    }

    match code {
        // The EC has already spun the fan up by the time we see this, so there
        // is nothing to do. Bit 3 of MSI_CPU[0] looked like a boost flag for one
        // measurement and then stayed high through thirty seconds of an audibly
        // running boost, so that was a coincidence; the tachometer in MSI_AP[2]
        // is what actually says the fan is flat out. See FINDINGS.md.
        COOLER_BOOST => {}

        // Windows handles these over HID. Acting on them would double the
        // keypress, which is why the kernel driver ignores them too.
        VOLUME_UP | VOLUME_DOWN => {}

        _ => {}
    }
}

// --- the notification area --------------------------------------------------

/// Sits in the notification area: tooltip with the live readings, a menu for
/// the log and the actions file, and a quit that disarms on the way out.
///
/// The subscription cannot share this thread — it blocks — so it gets a thread
/// and a connection of its own, and this one keeps the message loop running,
/// which a tray icon needs to receive anything at all.
fn run_in_tray() -> Result<()> {
    std::thread::spawn(|| {
        if let Err(e) = listen() {
            eprintln!("handler stopped: {e:#}");
        }
    });

    let readings = MenuItem::new("reading…", false, None);
    let open_log = MenuItem::new("Open the log", true, None);
    let edit_actions = MenuItem::new("Edit actions.txt", true, None);
    let quit = MenuItem::new("Disarm and quit", true, None);

    let menu = Menu::new();
    menu.append_items(&[
        &readings,
        &PredefinedMenuItem::separator(),
        &open_log,
        &edit_actions,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(tray_image()?)
        .with_tooltip("msi-hotkeys")
        .build()?;

    let wmi = connect()?;
    let mut next_refresh = Instant::now();

    loop {
        pump_messages();

        if Instant::now() >= next_refresh {
            let status = status_line(&wmi);
            let _ = tray.set_tooltip(Some(format!("msi-hotkeys — {status}")));
            readings.set_text(&status);
            next_refresh = Instant::now() + Duration::from_secs(2);
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == *quit.id() {
                disarm();
                return Ok(());
            } else if event.id == *open_log.id() {
                let _ = open_in_shell(&log_path());
            } else if event.id == *edit_actions.id() {
                let _ = open_in_shell(&actions_path());
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A tray icon drawn rather than shipped: a disc with a dark hub, which reads
/// at 16 pixels and needs no asset file next to the binary.
fn tray_image() -> Result<Icon> {
    const SIZE: u32 = 32;
    let centre = (SIZE as f32 - 1.0) / 2.0;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - centre;
            let dy = y as f32 - centre;
            let radius = dx.hypot(dy);

            let pixel = if radius > centre {
                [0, 0, 0, 0]
            } else if radius < centre * 0.3 {
                [24, 28, 34, 255]
            } else {
                [86, 184, 214, 255]
            };
            rgba.extend_from_slice(&pixel);
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| anyhow!("could not build the icon: {e}"))
}

/// Drains the thread's message queue. A tray icon is a window behind the
/// scenes, and without this it never hears about a click.
fn pump_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn open_in_shell(path: &PathBuf) -> Result<()> {
    // The empty argument is the window title that `start` would otherwise take
    // the path for.
    Command::new("cmd")
        .args(["/C", "start", "", &path.to_string_lossy()])
        .spawn()?;
    Ok(())
}

/// Disarms when the console window is closed or Ctrl+C pressed, then leaves.
fn install_console_handler() {
    use windows::core::BOOL;
    use windows::Win32::System::Console::SetConsoleCtrlHandler;

    unsafe extern "system" fn on_exit_signal(_kind: u32) -> BOOL {
        disarm();
        std::process::exit(0);
    }

    unsafe {
        let _ = SetConsoleCtrlHandler(Some(on_exit_signal), true);
    }
}

// --- readings ---------------------------------------------------------------

/// A screen that redraws itself once a second: the labelled readings, the fan
/// curve the controller is following, every EC byte raw, and the last few
/// button presses out of the log.
///
/// Deliberately a reader. Everything it shows is a query, so it can be left
/// open next to the running handler without the two interfering.
fn status_screen(wmi: &WMIConnection) -> Result<()> {
    // Windows consoles do not interpret escape sequences until asked to, and a
    // console that has not been asked prints them as text instead of acting on
    // them. If the request fails, the screen scrolls rather than redrawing,
    // which is worth having anyway.
    let vt = enable_virtual_terminal();

    loop {
        if vt {
            print!("\u{1B}[2J\u{1B}[H");
        } else {
            println!("\n{}", "-".repeat(78));
        }

        println!(
            "msi-hotkeys     {}     Ctrl+C to leave",
            chrono::Local::now().format("%H:%M:%S")
        );
        println!();
        println!("  {}", status_line(wmi));

        let armed = match read_byte(wmi, ARMING_CLASS, ARMING_PROPERTY, 0) {
            Some(1) => "armed".to_string(),
            Some(0) => "NOT armed — the buttons raise no events".to_string(),
            Some(other) => format!("unexpected value {other}"),
            None => "unreadable".to_string(),
        };
        println!("  channel: {armed}");

        println!();
        println!("  fan curve, as the controller has it:");
        for (label, class, property) in [("CPU", "MSI_CPU", "CPU"), ("GPU", "MSI_VGA", "VGA")] {
            if let Some(window) = read_window(wmi, class, property) {
                // Instances 5..10 hold the temperature points and 12..17 the
                // duty for each, read off the values the machine shipped with.
                let temps = window.get(5..11).unwrap_or_default();
                let duties = window.get(12..18).unwrap_or_default();
                let pairs: Vec<String> = temps
                    .iter()
                    .zip(duties)
                    .map(|(t, d)| format!("{t}C:{d}%"))
                    .collect();
                println!("    {label}  {}", pairs.join("  "));
            }
        }

        println!();
        println!("  EC windows:");
        for part in read_ec(wmi).split("  ") {
            if !part.is_empty() {
                println!("    {part}");
            }
        }

        println!();
        println!("  recent presses:");
        for line in recent_log_lines(6) {
            println!("    {line}");
        }

        std::io::stdout().flush().ok();
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Turns on escape-sequence handling for this console, reporting whether it
/// worked. A redirected or very old console will refuse, which is why the
/// caller has a fallback.
fn enable_virtual_terminal() -> bool {
    use windows::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        STD_OUTPUT_HANDLE,
    };

    unsafe {
        let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) else {
            return false;
        };
        let mut mode = Default::default();
        if GetConsoleMode(handle, &mut mode).is_err() {
            return false;
        }
        SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING).is_ok()
    }
}

/// The tail of the log, with the raw EC dumps left out so the screen stays
/// readable.
fn recent_log_lines(count: usize) -> Vec<String> {
    let text = match read_to_string(log_path()) {
        Ok(text) => text,
        Err(_) => return vec!["no log yet".to_string()],
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

/// The machine in one line: what can be stated with confidence, and the two EC
/// bytes whose behaviour has been watched long enough to name.
fn status_line(wmi: &WMIConnection) -> String {
    let mut facts = Vec::new();

    // MSI_CPU[1] tracked the real temperature across a boost: it fell from 60
    // to 42 while the fan was at maximum and climbed back afterwards. The label
    // is a reading of behaviour, not of documentation.
    if let Some(value) = read_byte(wmi, "MSI_CPU", "CPU", 1) {
        facts.push(format!("CPU ~{value}C"));
    }

    // MSI_AP[2] is the fan tachometer, which is why it looked like noise for
    // most of this project: it never holds still. Polled once a second it is
    // unmistakable — it sat at 137..138 for forty seconds of audible Cooler
    // Boost, slid down to 76 over thirteen seconds once the boost ended, then
    // held 77 at idle. The raw unit is unknown, so it is reported as it reads.
    //
    // A zero is not a stopped fan: the idle reading held 77 for thirty-six
    // consecutive samples, and the zeros turn up as single samples between
    // non-zero neighbours, so they are the register being read mid-update.
    match read_byte(wmi, "MSI_AP", "AP", 2) {
        Some(0) | None => facts.push("fan —".to_string()),
        Some(value) if value >= FAN_AT_MAXIMUM => {
            facts.push(format!("fan {value} (flat out — boost running)"));
        }
        Some(value) => facts.push(format!("fan {value}")),
    }

    // Mains and battery come from the documented providers instead.
    if let Ok(rows) = wmi.raw_query::<BatteryStatus>("SELECT * FROM BatteryStatus") {
        if let Some(battery) = rows.first() {
            facts.push(if battery.power_online {
                "on mains".to_string()
            } else {
                "on battery".to_string()
            });
            if battery.charging {
                facts.push("charging".to_string());
            } else if battery.discharging {
                facts.push("discharging".to_string());
            }
            facts.push(format!("{:.2}V", f64::from(battery.voltage) / 1000.0));
        }
    }

    if let Ok(cimv2) = WMIConnection::with_namespace_path(r"root\cimv2") {
        if let Ok(rows) = cimv2.raw_query::<Win32Battery>("SELECT * FROM Win32_Battery") {
            if let Some(percent) = rows.first().and_then(|b| b.estimated_charge_remaining) {
                facts.push(format!("{percent}%"));
            }
        }
    }

    if facts.is_empty() {
        "no readings".to_string()
    } else {
        facts.join("  ")
    }
}

/// Every byte of the EC windows on one line, for correlating a press with
/// whatever the controller changed at that exact moment.
fn read_ec(wmi: &WMIConnection) -> String {
    let mut windows = Vec::new();

    for (class, property) in EC_WINDOWS {
        let values = match read_window(wmi, class, property) {
            Some(values) => values,
            None => {
                windows.push(format!("{class}=<unreadable>"));
                continue;
            }
        };

        let bytes: Vec<String> = values.iter().map(|v| format!("{v:02X}")).collect();
        windows.push(format!("{class}=[{}]", bytes.join(" ")));
    }

    windows.join("  ")
}

/// One EC window, in instance order. Enumeration order is not guaranteed, so
/// the instances are sorted by the index in their name; comparing these windows
/// by position instead of by instance once produced a false lead, see
/// FINDINGS.md.
fn read_window(wmi: &WMIConnection, class: &str, property: &str) -> Option<Vec<u64>> {
    let rows: Vec<HashMap<String, Variant>> = wmi
        .raw_query(format!("SELECT InstanceName, {property} FROM {class}"))
        .ok()?;

    let mut indexed: Vec<(u32, u64)> = rows
        .iter()
        .filter_map(|row| {
            let index = match row.get("InstanceName") {
                Some(Variant::String(name)) => name.rsplit('_').next()?.parse().ok()?,
                _ => return None,
            };
            Some((index, as_u64(row.get(property)?)?))
        })
        .collect();
    indexed.sort_unstable();

    Some(indexed.into_iter().map(|(_, value)| value).collect())
}

fn read_byte(wmi: &WMIConnection, class: &str, property: &str, index: usize) -> Option<u64> {
    read_window(wmi, class, property)?.get(index).copied()
}

/// The windows are all UInt8 in the MOF, but the batteries are wider, so the
/// integer kinds are flattened rather than assumed.
fn as_u64(value: &Variant) -> Option<u64> {
    Some(match value {
        Variant::UI1(n) => u64::from(*n),
        Variant::UI2(n) => u64::from(*n),
        Variant::UI4(n) => u64::from(*n),
        Variant::UI8(n) => *n,
        Variant::I2(n) => u64::try_from(*n).ok()?,
        Variant::I4(n) => u64::try_from(*n).ok()?,
        _ => return None,
    })
}

fn describe(code: u32) -> &'static str {
    match code {
        COOLER_BOOST => "Cooler Boost",
        CENTER_KEY => "Center key",
        VOLUME_UP => "Volume up",
        VOLUME_DOWN => "Volume down",
        TOUCHPAD => "Touchpad (Fn+F3)",
        CAMERA_ONE | CAMERA_TWO => "Camera (Fn+F6)",
        // Everything else is a button whose purpose is not known yet; the log
        // keeps the raw code, which is what identifying it needs.
        _ => "unidentified",
    }
}

// --- actions ----------------------------------------------------------------

fn actions_path() -> PathBuf {
    config_dir().join("actions.txt")
}

/// Reads the button-to-command map. Unparsable lines are skipped rather than
/// fatal: a typo in a config file should not stop the buttons working.
fn load_actions() -> HashMap<u32, String> {
    let mut actions = HashMap::new();

    let text = match read_to_string(actions_path()) {
        Ok(text) => text,
        Err(_) => return actions,
    };

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((code, command)) = line.split_once('=') else {
            continue;
        };
        let code = code.trim();
        let command = command.trim();
        if command.is_empty() {
            continue;
        }

        let parsed = match code.strip_prefix("0x").or_else(|| code.strip_prefix("0X")) {
            Some(hex) => u32::from_str_radix(hex, 16),
            None => code.parse(),
        };
        if let Ok(code) = parsed {
            actions.insert(code, command.to_string());
        }
    }

    actions
}

/// Writes a commented example the first time, so there is something to edit.
fn write_action_template() -> Result<()> {
    let path = actions_path();
    if path.exists() {
        return Ok(());
    }
    create_dir_all(config_dir())?;

    let template = "\
# One line per button: <code> = <command line>
#
# The code is the one the log shows for that button. The command runs through
# cmd, so pipes and arguments work. This file is read again on every press, so
# changes take effect without restarting the handler.
#
# Mind that the handler runs elevated, and whatever it starts inherits that.
#
# 0x220029 = wt.exe
# 0x220004 = powershell -c \"Write-Host boost\"
";
    write(&path, template)?;
    Ok(())
}

/// Starts a mapped command without waiting for it.
fn run_action(command: &str) -> Result<()> {
    Command::new("cmd")
        .args(["/C", command])
        .spawn()
        .with_context(|| format!("could not start: {command}"))?;
    Ok(())
}

fn notify(title: &str, body: &str) {
    use tauri_winrt_notification::Toast;

    // An unpackaged Win32 program has no AppUserModelID of its own, and a toast
    // sent without one is silently dropped. Borrowing PowerShell's registered ID
    // is the usual way around that.
    if let Err(e) = Toast::new(Toast::POWERSHELL_APP_ID)
        .title(title)
        .text1(body)
        .show()
    {
        eprintln!("toast failed: {e}");
    }
}

// --- files ------------------------------------------------------------------

fn config_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("msi-hotkeys")
}

fn log_path() -> PathBuf {
    config_dir().join("events.log")
}

/// Appends one line to the log. A failure here must not take the handler down,
/// so it is reported and swallowed.
fn log(line: &str) {
    let path = log_path();
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
        .and_then(|mut f| f.write_all(stamped.as_bytes()));

    if let Err(e) = written {
        eprintln!("cannot write {}: {e}", path.display());
    }
}
