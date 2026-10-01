//! Everything that talks to the embedded controller through ACPI-WMI.
//!
//! All of it needs elevation: root\WMI is not readable otherwise.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use wmi::{Variant, WMIConnection};

/// The one EC byte that hands the buttons over to software, and the only thing
/// MSI's service does at startup. Measured on 2026-10-02: it reads 0 after a
/// boot in which no MSI software ran, and 1 once the service has started. While
/// it is 0 the controller deals with the buttons itself and raises no ACPI
/// event at all, so a subscription sits there silent.
pub const ARMING_CLASS: &str = "MSI_Software";
pub const ARMING_PROPERTY: &str = "Software";
pub const ARMING_INSTANCE: &str = r"ACPI\PNP0C14\0_0";

/// Where the tachometer sits when Cooler Boost is on. Measured plateau was
/// 137..138 against 77 at idle, so the threshold is well clear of both.
pub const FAN_MAX: u64 = 120;
pub const FAN_IDLE: u64 = 77;
pub const FAN_FULL: u64 = 138;

/// The classes that expose single-byte windows into EC RAM, and the property
/// each one carries its value in.
pub const EC_WINDOWS: [(&str, &str); 8] = [
    ("MSI_Software", "Software"),
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
pub struct MsiEvent {
    #[serde(rename = "MSIEvt")]
    pub code: u32,
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

/// What the interface shows. Every field is optional because a window can be
/// unreadable without that being fatal.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub armed: Option<bool>,
    pub cpu: Option<u64>,
    pub fan: Option<u64>,
    pub on_mains: Option<bool>,
    pub charging: bool,
    pub discharging: bool,
    pub volts: Option<f64>,
    pub percent: Option<u16>,
    /// Temperature points paired with the fan value for each, as the
    /// controller holds them.
    pub curve_cpu: Vec<(u64, u64)>,
    pub curve_gpu: Vec<(u64, u64)>,
    pub raw: Vec<(String, Vec<u64>)>,
    pub taken_at: String,
}

impl Snapshot {
    /// True while the tachometer sits at its plateau. Not a status bit: the
    /// controller exposes none, so this is a reading of the fan, not of what
    /// the user pressed.
    pub fn boost_running(&self) -> bool {
        self.fan.is_some_and(|value| value >= FAN_MAX)
    }
}

/// Each connection initialises COM for the thread it is made on, and is
/// `!Send`, so the compiler refuses to let one travel to another thread. Every
/// thread that needs WMI calls this for itself.
pub fn connect() -> Result<WMIConnection> {
    WMIConnection::with_namespace_path(r"root\WMI")
        .context(r"cannot connect to root\WMI — an elevated process is needed")
}

/// Writes the arming flag. Returns whether it had to write, so a caller can
/// tell "armed it" from "it was already armed".
pub fn set_arming(wmi: &WMIConnection, value: u8) -> Result<bool> {
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

/// Arms or disarms from a thread that holds no connection of its own.
pub fn set_armed_standalone(armed: bool) -> Result<bool> {
    let wmi = connect()?;
    set_arming(&wmi, u8::from(armed))
}

/// Reads everything the interface shows, in one pass.
pub fn snapshot(wmi: &WMIConnection) -> Snapshot {
    let mut snapshot = Snapshot {
        taken_at: chrono::Local::now().format("%H:%M:%S").to_string(),
        ..Default::default()
    };

    for (class, property) in EC_WINDOWS {
        if let Some(values) = read_window(wmi, class, property) {
            snapshot.raw.push((class.to_string(), values));
        }
    }

    // Read on its own rather than out of the windows above, so that whether the
    // interface knows the channel's state never depends on what the diagnostics
    // list happens to contain.
    snapshot.armed = read_window(wmi, ARMING_CLASS, ARMING_PROPERTY)
        .and_then(|window| window.first().copied())
        .map(|value| value == 1);
    snapshot.cpu = snapshot.window("MSI_CPU").and_then(|w| w.get(1).copied());
    // A zero is not a stopped fan: the idle reading held 77 for thirty-six
    // consecutive samples, and the zeros turn up as single samples between
    // non-zero neighbours, so they are the register being read mid-update.
    snapshot.fan = snapshot
        .window("MSI_AP")
        .and_then(|w| w.get(2).copied())
        .filter(|value| *value != 0);

    // Instances 5..10 hold the temperature points and 12..17 the fan value for
    // each, read off the values the machine shipped with.
    snapshot.curve_cpu = curve(snapshot.window("MSI_CPU"));
    snapshot.curve_gpu = curve(snapshot.window("MSI_VGA"));

    if let Ok(rows) = wmi.raw_query::<BatteryStatus>("SELECT * FROM BatteryStatus") {
        if let Some(battery) = rows.first() {
            snapshot.on_mains = Some(battery.power_online);
            snapshot.charging = battery.charging;
            snapshot.discharging = battery.discharging;
            snapshot.volts = Some(f64::from(battery.voltage) / 1000.0);
        }
    }

    if let Ok(cimv2) = WMIConnection::with_namespace_path(r"root\cimv2") {
        if let Ok(rows) = cimv2.raw_query::<Win32Battery>("SELECT * FROM Win32_Battery") {
            snapshot.percent = rows.first().and_then(|b| b.estimated_charge_remaining);
        }
    }

    snapshot
}

impl Snapshot {
    fn window(&self, class: &str) -> Option<&Vec<u64>> {
        self.raw
            .iter()
            .find(|(name, _)| name == class)
            .map(|(_, values)| values)
    }
}

fn curve(window: Option<&Vec<u64>>) -> Vec<(u64, u64)> {
    let Some(window) = window else {
        return Vec::new();
    };
    let temps = window.get(5..11).unwrap_or_default();
    let values = window.get(12..18).unwrap_or_default();
    temps
        .iter()
        .zip(values)
        .map(|(t, v)| (*t, *v))
        .collect()
}

/// One EC window, in instance order. Enumeration order is not guaranteed, so
/// the instances are sorted by the index in their name; comparing these windows
/// by position instead of by instance once produced a false lead, see
/// FINDINGS.md.
pub fn read_window(wmi: &WMIConnection, class: &str, property: &str) -> Option<Vec<u64>> {
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
