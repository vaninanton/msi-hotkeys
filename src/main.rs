// A windowed program: without this, Windows gives every run a console window.
// The `--console` and `--status` modes borrow the launching terminal instead,
// see `attach_parent_console`.
#![windows_subsystem = "windows"]

//! Handler for the hardware buttons of an MSI GL72 6QD.
//!
//! It replaces MSI's software completely: no SCM.exe, no `Micro Star SCM`
//! service, no ring-0 driver. It does need elevation: the ACPI-WMI classes
//! refuse reads without it, and the refusal arrives on the first read rather
//! than when the namespace is opened — so startup tests it by reading.
//!
//! Three jobs. It arms the event channel, which is the whole trick of this
//! project — see `Machine::set_armed`. It subscribes to the ACPI-WMI event the
//! buttons raise (`root\WMI` : `MSI_Event`, `MSIEvt` : `UInt32`). And it runs
//! whatever the user has mapped to a button in `actions.txt`.
//!
//! Started with no arguments it sits in the notification area. `--console` runs
//! it in the foreground instead, and `--status` opens a read-only screen that
//! can be left next to either.
//!
//! It deliberately does not touch the fan: Cooler Boost is carried out by the
//! controller itself, and the event is only a notification.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{create_dir_all, read_to_string, write, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use wmi::{Variant, WMIConnection};

/// Every button this machine has been seen to raise, with the name where there
/// is one. Keeping them in one table rather than in scattered constants is what
/// stops the lists drifting apart — an earlier version held them in two places
/// and left `MSI_Software` out of one, which made the channel state read as
/// unknown.
///
/// The codes are not all of the form `0x2200NN`: `0x220208`, `0x224957` and
/// `0x224B57` carry something in the middle byte too. The names come from the
/// scancode table in Linux' `msi-wmi.c`, which is keyed by our low byte: `0x08`
/// is `WIND_KEY_TOUCHPAD` ("Fn+F3 touchpad toggle") and `0x57` is
/// `WIND_KEY_CAMERA` ("Fn+F6 webcam toggle"). The camera appears under two
/// codes differing only in that middle byte, most likely on against off.
const BUTTONS: [(u32, &str); 7] = [
    (0x22_0004, "Cooler Boost"),
    (0x22_0029, "Center key"), // "MSI M-Center main menu" in the kernel
    (0x22_0021, "Volume down"),
    (0x22_0032, "Volume up"),
    (0x22_0208, "Touchpad (Fn+F3)"),
    (0x22_4957, "Camera (Fn+F6)"),
    (0x22_4B57, "Camera (Fn+F6)"),
];

/// The key next to the power button: it does nothing in hardware, which makes it
/// the one free button here, so it gets the fallback behaviour.
const CENTER_KEY: u32 = 0x22_0029;

/// The byte that hands the buttons over to software, and the only thing MSI's
/// service does at startup. Measured on 2026-10-02: it reads 0 after a boot in
/// which no MSI software ran, and 1 once the service has started. While it is 0
/// the controller deals with the buttons itself and raises no ACPI event at all,
/// so a subscription sits there silent — which is what made this channel look as
/// though it needed the service running.
const ARMING: Byte = Byte {
    class: "MSI_Software",
    property: "Software",
    index: 0,
};
const ARMING_INSTANCE: &str = r"ACPI\PNP0C14\0_0";

/// `MSI_CPU[1]` follows the CPU temperature in degrees. Named from behaviour,
/// not from documentation.
const CPU_TEMPERATURE: Byte = Byte {
    class: "MSI_CPU",
    property: "CPU",
    index: 1,
};

/// Named from the machine's own DSDT: `_WDG` maps each block to an object id,
/// the firmware's `WQ<id>` method lists the EC fields the block hands out, and
/// the EC region declares those fields by name. So these are the firmware
/// author's own addresses, not inferences from observed values. The decoding
/// tools are in tools/acpi; the map is docs/EC-MAP.md.
///
/// Where an address also appears in Linux' msi-ec or msi-laptop, the two agree:
/// `0x68` CPU temperature, `0x71` CPU fan, `0x80` GPU temperature, `0x89` GPU
/// fan, `0x33` bit 0 the fan automatics.
const FAN_AUTOMATICS: Byte = Byte { class: "MSI_CPU", property: "CPU", index: 0 };
const CPU_FAN: Byte = Byte { class: "MSI_CPU", property: "CPU", index: 2 };
const GPU_TEMPERATURE: Byte = Byte { class: "MSI_VGA", property: "VGA", index: 1 };
const GPU_FAN: Byte = Byte { class: "MSI_VGA", property: "VGA", index: 2 };

/// `MSI_Master_Battery` carries 16-bit words, not bytes: EC `0x31` holds the
/// status bits and the rest are low/high pairs from `0x38` up.
const BATTERY_FLAGS: Byte = Byte { class: "MSI_Master_Battery", property: "Master_Battery", index: 0 };
const BATTERY_PERCENT: Byte =
    Byte { class: "MSI_Master_Battery", property: "Master_Battery", index: 6 };
const BATTERY_FULL: Byte = Byte { class: "MSI_Master_Battery", property: "Master_Battery", index: 7 };
/// Signed: negative while discharging. This is the value whose sign used to
/// shift the whole window, because the reader dropped anything negative.
const BATTERY_CURRENT: Byte =
    Byte { class: "MSI_Master_Battery", property: "Master_Battery", index: 8 };
const BATTERY_LEFT: Byte = Byte { class: "MSI_Master_Battery", property: "Master_Battery", index: 9 };
const BATTERY_VOLTAGE_WORD: Byte =
    Byte { class: "MSI_Master_Battery", property: "Master_Battery", index: 10 };
const BATTERY_TEMPERATURE: Byte =
    Byte { class: "MSI_Master_Battery", property: "Master_Battery", index: 11 };

/// `MSI_Software[0]` is EC `0x2D` bit 0 — the flag Linux' msi-laptop calls
/// `MSI_STANDARD_EC_SCM_LOAD`, "set SCM load flag to disable BIOS fn key". The
/// gate this project went looking for is that documented bit, reached through
/// this window.
///
/// `MSI_AP[2]` is the fan tachometer by behaviour. Its address decodes as EC
/// `0xCD`, which no source names, and the rest of that block does not line up
/// with the named fields, so the AP mapping is the one part of the map not to
/// be trusted yet. The unit is unknown either way, so it is reported raw.
const FAN: Byte = Byte {
    class: "MSI_AP",
    property: "AP",
    index: 2,
};

/// The classes that expose single-byte windows into EC RAM, and the property
/// each one carries its value in.
const EC_WINDOWS: [(&str, &str); 8] = [
    ("MSI_Software", "Software"),
    ("MSI_AP", "AP"),
    ("MSI_Device", "Device"),
    ("MSI_System", "System"),
    ("MSI_Power", "Power"),
    ("MSI_CPU", "CPU"),
    ("MSI_VGA", "VGA"),
    ("MSI_Master_Battery", "Master_Battery"),
];

/// One named byte inside the EC windows.
struct Byte {
    class: &'static str,
    property: &'static str,
    index: usize,
}

/// An `MSI_Event` indication. The field name differs from the WMI property, so
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
        return status_screen(&Machine::open()?);
    }

    // Tested before the name is claimed, and tested by reading rather than by
    // connecting: opening the namespace succeeds without elevation and the
    // refusal only arrives on the first read of an ACPI class. A copy that cannot
    // read must not sit there holding the name, or a stray double-click would
    // block the working handler.
    if !Machine::open()?.can_read() {
        let message = "Классы ACPI-WMI не читаются: нужны права администратора";
        eprintln!("{message}");
        notify("msi-hotkeys", message);
        bail!(message);
    }

    // Two handlers would both arm the channel and both run whatever is mapped to
    // a pressed button, so the second start bows out.
    if !claim_single_instance() {
        println!("already running");
        notify("msi-hotkeys", "Обработчик уже запущен");
        return Ok(());
    }

    if wanted("--console") {
        // Ctrl+C would otherwise leave the channel armed, which is survivable
        // but untidy: the point of disarming is to hand the buttons back.
        install_console_handler();
    }
    if wanted("--console") {
        listen()
    } else {
        run_in_tray()
    }
}

// --- the machine ------------------------------------------------------------

/// The WMI connections and the operations on them.
///
/// Each connection initialises COM for the thread it is made on and is `!Send`,
/// so the compiler refuses to let one travel to another thread; every thread
/// that needs WMI opens its own `Machine`. Holding both namespaces here is the
/// point: the battery lives in `root\cimv2`, and opening that connection per
/// reading — as an earlier version did, once a second and once per keypress —
/// is pure waste.
struct Machine {
    wmi: WMIConnection,
    cimv2: Option<WMIConnection>,
}

impl Machine {
    fn open() -> Result<Self> {
        let wmi = WMIConnection::with_namespace_path(r"root\WMI")
            .context(r"cannot reach root\WMI — the ACPI-WMI classes are not readable here")?;
        // Optional on purpose: without it the battery percentage is missing and
        // everything else still works.
        let cimv2 = WMIConnection::with_namespace_path(r"root\cimv2").ok();
        Ok(Self { wmi, cimv2 })
    }

    /// Whether the ACPI classes answer at all. Connecting to the namespace works
    /// without elevation; the refusal — `WBEM_E_ACCESS_DENIED`, `0x8004_1003` —
    /// comes on the first read, so reading is the only honest way to test it.
    fn can_read(&self) -> bool {
        self.read_window(ARMING.class, ARMING.property).is_some()
    }

    /// Every EC window in one pass: one read serves the log line, the raw dump
    /// and the channel state, so the three cannot drift apart.
    fn read(&self) -> Ec {
        let windows = EC_WINDOWS
            .iter()
            .map(|(class, property)| (*class, self.read_window(class, property)))
            .collect();
        Ec { windows }
    }

    /// One EC window, in instance order. Enumeration order is not guaranteed, so
    /// the instances are sorted by the index in their name; comparing these
    /// windows by position instead of by instance once produced a false lead,
    /// see docs/FINDINGS.md.
    fn read_window(&self, class: &str, property: &str) -> Option<Vec<i64>> {
        let rows: Vec<HashMap<String, Variant>> = self
            .wmi
            .raw_query(format!("SELECT InstanceName, {property} FROM {class}"))
            .ok()?;

        let mut indexed: Vec<(u32, i64)> = rows
            .iter()
            .filter_map(|row| {
                let Some(Variant::String(name)) = row.get("InstanceName") else {
                    return None;
                };
                let index = name.rsplit('_').next()?.parse().ok()?;
                Some((index, as_i64(row.get(property)?)?))
            })
            .collect();
        indexed.sort_unstable();

        Some(indexed.into_iter().map(|(_, value)| value).collect())
    }

    fn power(&self) -> Option<Power> {
        let rows: Vec<BatteryStatus> = self
            .wmi
            .raw_query("SELECT * FROM BatteryStatus")
            .ok()?;
        let battery = rows.first()?;

        let percent = self.cimv2.as_ref().and_then(|cimv2| {
            let rows: Vec<Win32Battery> = cimv2.raw_query("SELECT * FROM Win32_Battery").ok()?;
            rows.first()?.estimated_charge_remaining
        });

        Some(Power {
            on_mains: battery.power_online,
            charging: battery.charging,
            discharging: battery.discharging,
            volts: f64::from(battery.voltage) / 1000.0,
            percent,
        })
    }

    /// Writes the arming flag. Returns whether it had to write, so a run on an
    /// already-armed machine can say so.
    fn set_armed(&self, armed: bool) -> Result<bool> {
        let value = u8::from(armed);
        // A WMI object path treats the backslash as an escape, so each one in
        // the instance name is doubled.
        let escaped = ARMING_INSTANCE.replace('\\', r"\\");
        let path = format!("{}.InstanceName=\"{escaped}\"", ARMING.class);

        let object = self
            .wmi
            .get_object(&path)
            .with_context(|| format!("no such instance: {path}"))?;

        if let Ok(Variant::UI1(current)) = object.get_property(ARMING.property) {
            if current == value {
                return Ok(false);
            }
        }

        object
            .put_property(ARMING.property, Variant::UI1(value))
            .context("could not set the property on the local copy")?;
        self.wmi
            .put_instance(&object)
            .context("the instance update was rejected")?;

        // The EC can refuse silently, so the value is read back rather than
        // trusted.
        let after = self.wmi.get_object(&path)?.get_property(ARMING.property)?;
        if !matches!(after, Variant::UI1(written) if written == value) {
            bail!("the write did not stick, reads back as {after:?}");
        }

        Ok(true)
    }
}

/// One pass over the EC windows, with the bytes whose meaning is known given
/// names. Everything is optional because a window can be unreadable without
/// that being fatal.
struct Ec {
    windows: Vec<(&'static str, Option<Vec<i64>>)>,
}

impl Ec {
    fn window(&self, class: &str) -> Option<&[i64]> {
        self.windows
            .iter()
            .find(|(name, _)| *name == class)
            .and_then(|(_, values)| values.as_deref())
    }

    fn byte(&self, byte: &Byte) -> Option<i64> {
        self.window(byte.class)?.get(byte.index).copied()
    }

    fn armed(&self) -> Option<bool> {
        self.byte(&ARMING).map(|value| value == 1)
    }

    /// A zero is not a stopped fan: the idle reading held 77 for thirty-six
    /// consecutive samples, and the zeros turn up as single samples between
    /// non-zero neighbours, so they are the register being read mid-update.
    fn fan(&self) -> Option<i64> {
        self.byte(&FAN).filter(|value| *value != 0)
    }

    /// Temperature points paired with the fan value for each, as the controller
    /// holds them: instances 5..10 are the temperatures and 12..17 the values.
    fn curve(&self, class: &str) -> Vec<(i64, i64)> {
        let Some(window) = self.window(class) else {
            return Vec::new();
        };
        let temps = window.get(5..11).unwrap_or_default();
        let values = window.get(12..18).unwrap_or_default();
        temps.iter().copied().zip(values.iter().copied()).collect()
    }

    /// Every byte raw, for correlating a press with whatever the controller
    /// changed at that moment. This dump is what identifying the remaining
    /// buttons and bytes will be done from.
    fn dump(&self) -> String {
        let mut out = String::new();
        for (class, values) in &self.windows {
            if !out.is_empty() {
                out.push_str("  ");
            }
            match values {
                Some(values) => {
                    let _ = write!(out, "{class}=[");
                    for (position, value) in values.iter().enumerate() {
                        let separator = if position == 0 { "" } else { " " };
                        let _ = write!(out, "{separator}{value:02X}");
                    }
                    out.push(']');
                }
                None => {
                    let _ = write!(out, "{class}=<unreadable>");
                }
            }
        }
        out
    }
}

struct Power {
    on_mains: bool,
    charging: bool,
    discharging: bool,
    volts: f64,
    percent: Option<u16>,
}

/// How many lines the tray menu shows.
const READING_LINES: usize = 10;

/// The readings as separate lines, for the tray menu. In Russian, like
/// everything else the user sees; the console and the log stay in English.
fn readings(ec: &Ec, power: Option<&Power>) -> [String; READING_LINES] {
    // One value per line, and always the same lines: a menu that changes shape
    // depending on what answered is harder to read than one with a dash in it.
    let dash = |label: &str| format!("{label}: —");

    [
        match ec.armed() {
            Some(true) => "Канал: вооружён".to_string(),
            Some(false) => "Канал: не вооружён".to_string(),
            None => dash("Канал"),
        },
        match (ec.byte(&CPU_TEMPERATURE), ec.byte(&CPU_FAN)) {
            (Some(t), Some(f)) => format!("CPU: {t} °C · вентилятор {f}"),
            (Some(t), None) => format!("CPU: {t} °C"),
            _ => dash("CPU"),
        },
        // The discrete card is powered down at idle, and then its temperature
        // reads zero rather than being absent.
        match (ec.byte(&GPU_TEMPERATURE), ec.byte(&GPU_FAN)) {
            (Some(0), _) => "GPU: спит".to_string(),
            (Some(t), Some(f)) => format!("GPU: {t} °C · вентилятор {f}"),
            (Some(t), None) => format!("GPU: {t} °C"),
            _ => dash("GPU"),
        },
        // EC 0x33 bit 0, which msi-laptop documents as the fan automatics:
        // zero means the controller drives the fan flat out.
        match ec.byte(&FAN_AUTOMATICS) {
            Some(value) if value & 1 == 0 => "Вентилятор: на максимуме".to_string(),
            Some(_) => "Вентилятор: по кривой".to_string(),
            None => dash("Вентилятор"),
        },
        ec.fan()
            .map_or_else(|| dash("Тахометр"), |value| format!("Тахометр: {value}")),
        power.map_or_else(
            || dash("Питание"),
            |power| {
                format!(
                    "Питание: {}",
                    if power.on_mains { "от сети" } else { "от батареи" }
                )
            },
        ),
        // Percentage and current come from the controller; Windows agrees to
        // within a point, and the current carries the sign that says which way
        // the charge is going.
        match (
            ec.byte(&BATTERY_PERCENT)
                .or_else(|| power.and_then(|p| p.percent).map(i64::from)),
            ec.byte(&BATTERY_CURRENT),
        ) {
            (Some(percent), Some(current)) if current != 0 => {
                format!("Заряд: {percent} % · {current} мА")
            }
            (Some(percent), _) => format!("Заряд: {percent} %"),
            _ => dash("Заряд"),
        },
        // Battery figures straight from the controller, where the kernel's own
        // field names say what they are: MVO the voltage, MTE the temperature in
        // tenths of a kelvin, MRC and MFC the remaining and full capacity.
        match (
            ec.byte(&BATTERY_VOLTAGE_WORD),
            ec.byte(&BATTERY_TEMPERATURE),
        ) {
            (Some(mv), Some(raw)) => format!(
                "Батарея: {:.2} В · {:.1} °C",
                millivolts(mv),
                kelvin_tenths(raw)
            ),
            (Some(mv), None) => format!("Батарея: {:.2} В", millivolts(mv)),
            _ => power.map_or_else(
                || dash("Батарея"),
                |power| format!("Батарея: {:.2} В", power.volts),
            ),
        },
        // The unit is not established, so the two figures are shown as they read
        // and only their ratio is meaningful.
        match (ec.byte(&BATTERY_LEFT), ec.byte(&BATTERY_FULL)) {
            (Some(left), Some(full)) if full > 0 => {
                format!("Ёмкость: {left} из {full} мА·ч")
            }
            _ => dash("Ёмкость"),
        },
        match ec.byte(&BATTERY_FLAGS) {
            Some(flags) => {
                let mut state = Vec::new();
                if flags & 0x01 != 0 {
                    state.push("установлена");
                }
                if flags & 0x02 != 0 {
                    state.push("заряжается");
                }
                if flags & 0x04 != 0 {
                    state.push("разряжается");
                }
                if flags & 0x08 != 0 {
                    state.push("заряжена");
                }
                if state.is_empty() {
                    dash("Состояние")
                } else {
                    format!("Состояние: {}", state.join(" · "))
                }
            }
            None => dash("Состояние"),
        },
    ]
}

/// EC words are 16-bit, so the conversions go through f64 without a narrowing
/// cast: a reading wider than expected should look wrong, not silently wrap.
fn millivolts(value: i64) -> f64 {
    f64::from(i32::try_from(value).unwrap_or(i32::MAX)) / 1000.0
}

fn kelvin_tenths(value: i64) -> f64 {
    f64::from(i32::try_from(value).unwrap_or(i32::MAX)) / 10.0 - 273.15
}

/// The machine in one line, from a reading already taken.
fn summary(ec: &Ec, power: Option<&Power>) -> String {
    let mut facts = Vec::new();

    if let Some(value) = ec.byte(&CPU_TEMPERATURE) {
        facts.push(format!("CPU ~{value}C"));
    }

    // Reported as it reads. The unit is unknown and the controller exposes no
    // boost state, so any threshold here would be an interpretation, not a fact.
    facts.push(match ec.fan() {
        Some(value) => format!("fan {value}"),
        None => "fan —".to_string(),
    });

    if let Some(power) = power {
        facts.push(if power.on_mains { "on mains" } else { "on battery" }.to_string());
        if power.charging {
            facts.push("charging".to_string());
        } else if power.discharging {
            facts.push("discharging".to_string());
        }
        facts.push(format!("{:.2}V", power.volts));
        if let Some(percent) = power.percent {
            facts.push(format!("{percent}%"));
        }
    }

    if facts.is_empty() {
        "no readings".to_string()
    } else {
        facts.join("  ")
    }
}

fn button_name(code: u32) -> &'static str {
    BUTTONS
        .iter()
        .find(|(known, _)| *known == code)
        .map_or("unidentified", |(_, name)| *name)
}

fn as_i64(value: &Variant) -> Option<i64> {
    // The windows are all UInt8 in the MOF, but the batteries are wider, so the
    // integer kinds are flattened rather than assumed.
    Some(match value {
        Variant::UI1(n) => i64::from(*n),
        Variant::UI2(n) => i64::from(*n),
        Variant::UI4(n) => i64::from(*n),
        Variant::UI8(n) => i64::try_from(*n).ok()?,
        Variant::I1(n) => i64::from(*n),
        Variant::I2(n) => i64::from(*n),
        Variant::I4(n) => i64::from(*n),
        Variant::I8(n) => *n,
        _ => return None,
    })
}

// --- the handler ------------------------------------------------------------

/// Arms the channel and then blocks, handling events until the process ends.
fn listen() -> Result<()> {
    let machine = Machine::open()?;

    match machine.set_armed(true) {
        Ok(true) => println!("armed the channel ({} -> 1)", ARMING.class),
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
    for indication in machine.wmi.notification::<MsiEvent>()? {
        match indication {
            Ok(event) => handle(&machine, event.code),
            Err(e) => eprintln!("subscription error: {e:#}"),
        }
    }

    Ok(())
}

/// Everything that happens on a button press.
///
/// Nothing is done about the fan: Cooler Boost is carried out by the controller
/// before the event even arrives, and the volume keys are handled by Windows
/// over HID, where acting on them would double the keypress — which is why the
/// kernel driver ignores them too.
fn handle(machine: &Machine, code: u32) {
    let name = button_name(code);
    println!("0x{code:06X}  {name}");

    // One reading serves both log lines. The old version took eleven WMI round
    // trips per press to produce the same two.
    let ec = machine.read();
    let status = summary(&ec, machine.power().as_ref());
    log(&format!("0x{code:06X}  {name:16}  {status}"));
    log(&format!("{:26}{}", "", ec.dump()));

    // The actions file is read per press rather than at startup, so editing it
    // takes effect without restarting the handler.
    match load_actions().get(&code) {
        Some(command) => match run_action(command) {
            Ok(()) => log(&format!("{:26}ran: {command}", "")),
            Err(e) => log(&format!("{:26}FAILED to run {command}: {e:#}", "")),
        },
        // Nothing mapped yet, so the free button at least reports the machine.
        None if code == CENTER_KEY => notify("msi-hotkeys", &status),
        None => {}
    }
}

/// Hands the buttons back to the firmware. Called on the way out, from whichever
/// thread is leaving, so it opens a connection of its own.
fn disarm() {
    match Machine::open().and_then(|machine| machine.set_armed(false)) {
        Ok(true) => println!("disarmed the channel"),
        Ok(false) => println!("channel was not armed"),
        Err(e) => eprintln!("could not disarm: {e:#}"),
    }
}

// --- the notification area --------------------------------------------------

/// Sits in the notification area: tooltip with the live readings, a menu for the
/// log and the actions file, and a quit that disarms on the way out.
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

    // The readings are polled on a thread of their own and this one only copies
    // the strings into the menu. A WMI connection made on the thread that owns
    // the tray icon does not work: the tray initialises COM there as a
    // single-threaded apartment, and queries on a connection made afterwards
    // return nothing while the connection itself appears to open.
    let published = spawn_poller();

    // Disabled items: a native menu has no other way to show a line of text, and
    // greyed out is also the honest look for something that is a reading rather
    // than a command.
    let lines: Vec<MenuItem> = (0..READING_LINES)
        .map(|_| MenuItem::new("…", false, None))
        .collect();
    let quit = MenuItem::new("Выход", true, None);

    let menu = Menu::new();
    for line in &lines {
        menu.append(line)?;
    }
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit)?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(tray_image()?)
        .with_tooltip("msi-hotkeys")
        .build()?;

    let mut shown = Published::default();

    loop {
        pump_messages();

        // Copying strings, so this can run often; the cost is on the polling
        // thread, which sets its own pace.
        let latest = published.lock().unwrap().clone();
        if latest != shown {
            for (item, text) in lines.iter().zip(&latest.lines) {
                item.set_text(text);
            }
            let _ = tray.set_tooltip(Some(format!("msi-hotkeys — {}", latest.tooltip)));
            shown = latest;
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == *quit.id() {
                disarm();
                return Ok(());
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What the polling thread hands to the tray: the menu lines and the tooltip,
/// already formatted.
#[derive(Clone, Default, PartialEq, Eq)]
struct Published {
    lines: Vec<String>,
    tooltip: String,
}

/// Reads the machine once a second on a thread of its own and publishes the
/// formatted result. The menu never touches WMI.
fn spawn_poller() -> Arc<Mutex<Published>> {
    let published = Arc::new(Mutex::new(Published {
        lines: vec!["…".to_string(); READING_LINES],
        tooltip: "читаю…".to_string(),
    }));

    let writer = Arc::clone(&published);
    std::thread::spawn(move || {
        let machine = match Machine::open() {
            Ok(machine) => machine,
            Err(e) => {
                let mut published = writer.lock().unwrap();
                published.lines = vec![format!("{e:#}")];
                published.tooltip = format!("{e:#}");
                return;
            }
        };

        loop {
            let ec = machine.read();
            let power = machine.power();
            let next = Published {
                lines: readings(&ec, power.as_ref()).to_vec(),
                tooltip: summary(&ec, power.as_ref()),
            };
            *writer.lock().unwrap() = next;
            std::thread::sleep(Duration::from_secs(1));
        }
    });

    published
}

/// A tray icon drawn rather than shipped: a disc with a dark hub, which reads at
/// 16 pixels and needs no asset file next to the binary.
fn tray_image() -> Result<Icon> {
    const SIZE: u32 = 32;
    let centre = f64::from(SIZE - 1) / 2.0;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);

    for y in 0..SIZE {
        for x in 0..SIZE {
            let radius = (f64::from(x) - centre).hypot(f64::from(y) - centre);
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

// --- the status screen ------------------------------------------------------

/// A screen that redraws itself once a second: the labelled readings, the fan
/// curve the controller is following, every EC byte raw, and the last few button
/// presses out of the log.
///
/// Deliberately a reader, so it can be left open beside the running handler.
fn status_screen(machine: &Machine) -> Result<()> {
    // Windows consoles do not interpret escape sequences until asked to, and one
    // that has not been asked prints them as text instead of acting on them. If
    // the request fails the screen scrolls rather than redrawing, which is worth
    // having anyway.
    let vt = enable_virtual_terminal();

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
        println!("  {}", summary(&ec, machine.power().as_ref()));
        println!(
            "  channel: {}",
            match ec.armed() {
                Some(true) => "armed",
                Some(false) => "NOT armed — the buttons raise no events",
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
        for (class, values) in &ec.windows {
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
        for line in recent_log_lines(6) {
            println!("    {line}");
        }

        std::io::stdout().flush().ok();
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The tail of the log, with the raw EC dumps left out so the screen stays
/// readable.
fn recent_log_lines(count: usize) -> Vec<String> {
    let Ok(text) = read_to_string(log_path()) else {
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

// --- actions ----------------------------------------------------------------

/// Reads the button-to-command map. Unparsable lines are skipped rather than
/// fatal: a typo in a config file should not stop the buttons working.
fn load_actions() -> HashMap<u32, String> {
    let mut actions = HashMap::new();

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

fn parse_code(text: &str) -> Option<u32> {
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// Writes a commented example the first time, so there is something to edit.
fn write_action_template() -> Result<()> {
    let path = actions_path();
    if path.exists() {
        return Ok(());
    }
    create_dir_all(config_dir())?;

    write(
        &path,
        "\
# One line per button: <code> = <command line>
#
# The code is the one the log shows for that button. The command runs through
# cmd, so pipes and arguments work. This file is read again on every press, so
# changes take effect without restarting the handler.
#
# Mind that the handler runs elevated, and whatever it starts inherits that.
#
# 0x220029 = wt.exe
",
    )?;
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
    let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
    base.join("msi-hotkeys")
}

fn actions_path() -> PathBuf {
    config_dir().join("actions.txt")
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
        .and_then(|mut file| file.write_all(stamped.as_bytes()));

    if let Err(e) = written {
        eprintln!("cannot write {}: {e}", path.display());
    }
}

// --- Windows odds and ends --------------------------------------------------

/// Takes the name that marks "a handler is running", reporting whether it was
/// free. The handle is deliberately never closed: the name must stay taken for
/// as long as this process runs, and Windows releases it when the process ends.
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
/// Without re-opening `CONOUT$` the handles stay invalid and nothing is printed.
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

/// Turns on escape-sequence handling for this console, reporting whether it
/// worked. A redirected or very old console will refuse, which is why the caller
/// has a fallback.
fn enable_virtual_terminal() -> bool {
    use windows::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, CONSOLE_MODE,
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_OUTPUT_HANDLE,
    };

    unsafe {
        let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) else {
            return false;
        };
        let mut mode = CONSOLE_MODE::default();
        if GetConsoleMode(handle, &raw mut mode).is_err() {
            return false;
        }
        SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING).is_ok()
    }
}

/// Drains the thread's message queue. A tray icon is a window behind the scenes,
/// and without this it never hears about a click.
fn pump_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}
