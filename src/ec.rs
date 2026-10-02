//! Reading the embedded controller through MSI's ACPI-WMI blocks.
//!
//! The machine exposes nine `MSI_*` classes in `root\WMI`. Each is a window onto
//! a handful of EC bytes: the firmware's `_WDG` block maps a class GUID to an
//! object id, its `WQ<id>` method lists the EC fields the window hands out, and
//! the `EC__` operation region declares those fields by name. Everything in
//! [`map`] below was decoded from this machine's own ACPI tables; the tools are
//! in `tools/acpi` and the result is written up in `docs/EC-MAP.md`.
//!
//! Reading needs administrator rights. Opening the namespace does not, which is
//! the trap: the refusal only arrives on the first read, so [`Machine::can_read`]
//! tests by reading.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use wmi::{Variant, WMIConnection};

/// One named value inside a window.
#[derive(Clone, Copy, Debug)]
pub struct Byte {
    pub class: &'static str,
    pub property: &'static str,
    pub index: usize,
}

impl Byte {
    const fn new(class: &'static str, property: &'static str, index: usize) -> Self {
        Self {
            class,
            property,
            index,
        }
    }
}

/// The bytes this program uses, each with the EC address it decodes to.
///
/// Where an address also appears in Linux' `msi-ec` or `msi-laptop`, the two
/// agree. Two of their addresses do not work here: `0x71` and `0x89`, which
/// `msi-ec` calls the real fan speeds, read zero on this firmware even with
/// Cooler Boost audibly running.
pub mod map {
    use super::Byte;

    /// EC `0x2D` bit 0 — the flag Linux' `msi-laptop` calls
    /// `MSI_STANDARD_EC_SCM_LOAD`: "set SCM load flag to disable BIOS fn key".
    /// While it is 0 the controller handles the buttons itself and raises no
    /// ACPI event, so a subscription sits there silent.
    pub const HANDOVER: Byte = Byte::new("MSI_Software", "Software", 0);
    /// EC `0x68`.
    pub const CPU_TEMPERATURE: Byte = Byte::new("MSI_CPU", "CPU", 1);
    /// EC `0x80`. Reads zero while the discrete card is powered down.
    pub const GPU_TEMPERATURE: Byte = Byte::new("MSI_VGA", "VGA", 1);
    /// EC `0x33` bit 0: zero means the controller is driving the fan flat out
    /// rather than following its curve.
    pub const FAN_CURVE_ACTIVE: Byte = Byte::new("MSI_CPU", "CPU", 0);
    /// EC `0xE4` bit 1.
    pub const TURBO: Byte = Byte::new("MSI_System", "System", 0);
    /// EC `0x2E`: the state of the wireless parts. Bit numbers are
    /// `msi-laptop`'s.
    pub const DEVICE_STATE: Byte = Byte::new("MSI_Device", "Device", 0);
    /// EC `0x2F`: which of those parts the machine has at all.
    pub const DEVICE_PRESENT: Byte = Byte::new("MSI_Device", "Device", 1);

    /// The fan reading, by behaviour rather than by name: it plateaus under
    /// Cooler Boost and slides back down afterwards through a dozen
    /// intermediate values, which a commanded duty would not do. Its unit is
    /// unknown, so it is reported raw. Its address decodes as EC `0xCD`, but the
    /// `MSI_AP` window is the one part of the map that does not line up with the
    /// named fields, so take the address with salt.
    pub const FAN: Byte = Byte::new("MSI_AP", "AP", 2);

    /// `MSI_Master_Battery` carries 16-bit words rather than bytes.
    pub mod battery {
        use super::Byte;

        /// EC `0x31`: bit 0 present, 1 charging, 2 discharging, 3 full.
        pub const FLAGS: Byte = Byte::new("MSI_Master_Battery", "Master_Battery", 0);
        pub const PERCENT: Byte = Byte::new("MSI_Master_Battery", "Master_Battery", 6);
        /// Full charge capacity, mA·h.
        pub const FULL: Byte = Byte::new("MSI_Master_Battery", "Master_Battery", 7);
        /// Current in mA, negative while discharging. The sign matters beyond
        /// the display: a reader that drops negative values shifts every later
        /// index in the window, which is a bug this project has already had.
        pub const CURRENT: Byte = Byte::new("MSI_Master_Battery", "Master_Battery", 8);
        /// Remaining capacity, mA·h.
        pub const LEFT: Byte = Byte::new("MSI_Master_Battery", "Master_Battery", 9);
        /// Millivolts.
        pub const VOLTAGE: Byte = Byte::new("MSI_Master_Battery", "Master_Battery", 10);
        /// Tenths of a kelvin.
        pub const TEMPERATURE: Byte = Byte::new("MSI_Master_Battery", "Master_Battery", 11);

        pub const PRESENT: i64 = 1 << 0;
        pub const CHARGING: i64 = 1 << 1;
        pub const DISCHARGING: i64 = 1 << 2;
        pub const FULL_FLAG: i64 = 1 << 3;
    }

    /// `MSI_Software[6..34]`: the 28-byte firmware version string. The mainline
    /// documentation of MSI's `Get_EC()` method describes the same shape.
    pub const FIRMWARE_RANGE: std::ops::Range<usize> = 6..34;

    /// Bit masks within [`DEVICE_STATE`] and [`DEVICE_PRESENT`].
    pub const DEVICES: [(i64, &str); 4] = [
        (1 << 0, "Bluetooth"),
        (1 << 1, "камера"),
        (1 << 3, "WLAN"),
        (1 << 4, "3G"),
    ];

    /// Bit 1 of [`TURBO`].
    pub const TURBO_BIT: i64 = 1 << 1;
    /// Bit 0 of [`FAN_CURVE_ACTIVE`].
    pub const FAN_CURVE_BIT: i64 = 1 << 0;

    /// Every window the program reads, with the property each carries its value
    /// in. `MSI_Slave_Battery` is left out: all fourteen of its words are zero,
    /// this machine has one battery.
    pub const WINDOWS: [(&str, &str); 8] = [
        ("MSI_Software", "Software"),
        ("MSI_AP", "AP"),
        ("MSI_Device", "Device"),
        ("MSI_System", "System"),
        ("MSI_Power", "Power"),
        ("MSI_CPU", "CPU"),
        ("MSI_VGA", "VGA"),
        ("MSI_Master_Battery", "Master_Battery"),
    ];

    /// Instances 5..11 of a fan window hold the temperature points and 12..18
    /// the fan value for each.
    pub const CURVE_TEMPERATURES: std::ops::Range<usize> = 5..11;
    pub const CURVE_VALUES: std::ops::Range<usize> = 12..18;
}

/// An `MSI_Event` indication. The field name differs from the WMI property, so
/// serde is told the wire name; `rename` on the struct makes the `wmi` crate
/// build `SELECT * FROM MSI_Event` for us.
#[derive(Deserialize, Debug)]
#[serde(rename = "MSI_Event")]
pub struct Event {
    #[serde(rename = "MSIEvt")]
    pub code: u32,
}

/// Mains and battery state from the documented WDM provider, kept as a
/// fallback for the figures the EC also reports.
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

/// What Windows says about power, as distinct from what the controller says.
#[derive(Clone, Copy, Debug)]
pub struct Power {
    pub on_mains: bool,
    pub charging: bool,
    pub discharging: bool,
    pub volts: f64,
    pub percent: Option<u16>,
}

/// The connections, and the operations on them.
///
/// Each connection initialises COM for the thread it is made on and is `!Send`,
/// so the compiler refuses to let one travel to another thread: every thread
/// that needs WMI opens its own `Machine`. One caveat learned the hard way — a
/// connection made on the thread that owns the tray icon returns nothing,
/// because the tray initialises COM there as a single-threaded apartment. Give
/// the readings a thread of their own.
pub struct Machine {
    wmi: WMIConnection,
    cimv2: Option<WMIConnection>,
}

impl Machine {
    pub fn open() -> Result<Self> {
        let wmi = WMIConnection::with_namespace_path(r"root\WMI")
            .context(r"cannot reach root\WMI — the ACPI-WMI classes are not readable here")?;
        // Optional: without it the battery percentage loses its fallback and
        // everything else still works.
        let cimv2 = WMIConnection::with_namespace_path(r"root\cimv2").ok();
        Ok(Self { wmi, cimv2 })
    }

    /// Whether the ACPI classes answer at all. Connecting succeeds without
    /// administrator rights and the refusal — `WBEM_E_ACCESS_DENIED`,
    /// `0x8004_1003` — comes on the first read, so reading is the only honest
    /// way to test it.
    pub fn can_read(&self) -> bool {
        self.window(map::HANDOVER.class, map::HANDOVER.property)
            .is_some()
    }

    /// Every window in one pass: one read serves the readings, the raw dump and
    /// the channel state, so the three cannot drift apart.
    pub fn read(&self) -> Ec {
        let windows = map::WINDOWS
            .iter()
            .map(|(class, property)| (*class, self.window(class, property)))
            .collect();
        Ec { windows }
    }

    /// One window, in instance order. Enumeration order is not guaranteed, so
    /// instances are sorted by the index in their name; comparing windows by
    /// position rather than by instance once produced a false lead.
    fn window(&self, class: &str, property: &str) -> Option<Vec<i64>> {
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

    pub fn power(&self) -> Option<Power> {
        let rows: Vec<BatteryStatus> = self.wmi.raw_query("SELECT * FROM BatteryStatus").ok()?;
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

    /// Hands the buttons to software, or back to the firmware. Returns whether it
    /// had to write.
    pub fn set_handover(&self, to_software: bool) -> Result<bool> {
        let value = u8::from(to_software);
        // A WMI object path treats the backslash as an escape, so each one in
        // the instance name is doubled.
        let escaped = HANDOVER_INSTANCE.replace('\\', r"\\");
        let path = format!("{}.InstanceName=\"{escaped}\"", map::HANDOVER.class);

        let object = self
            .wmi
            .get_object(&path)
            .with_context(|| format!("no such instance: {path}"))?;

        if let Ok(Variant::UI1(current)) = object.get_property(map::HANDOVER.property) {
            if current == value {
                return Ok(false);
            }
        }

        object
            .put_property(map::HANDOVER.property, Variant::UI1(value))
            .context("could not set the property on the local copy")?;
        self.wmi
            .put_instance(&object)
            .context("the instance update was rejected")?;

        // The controller can refuse silently, so the value is read back.
        let after = self
            .wmi
            .get_object(&path)?
            .get_property(map::HANDOVER.property)?;
        if !matches!(after, Variant::UI1(written) if written == value) {
            bail!("the write did not stick, reads back as {after:?}");
        }

        Ok(true)
    }

    /// Subscribes to the button events. The iterator blocks until one arrives,
    /// so a loop over it costs nothing while idle: no polling, no timer.
    pub fn events(&self) -> Result<impl Iterator<Item = wmi::WMIResult<Event>> + '_> {
        self.wmi.notification::<Event>().map_err(Into::into)
    }
}

const HANDOVER_INSTANCE: &str = r"ACPI\PNP0C14\0_0";

/// One pass over the windows. Everything is optional because a window can be
/// unreadable without that being fatal.
#[derive(Clone, Debug, Default)]
pub struct Ec {
    windows: Vec<(&'static str, Option<Vec<i64>>)>,
}

impl Ec {
    /// Builds a reading from values, for tests and for anything that needs an Ec
    /// without a machine to read it from.
    #[cfg(test)]
    pub const fn from_windows(windows: Vec<(&'static str, Option<Vec<i64>>)>) -> Self {
        Self { windows }
    }

    pub fn window(&self, class: &str) -> Option<&[i64]> {
        self.windows
            .iter()
            .find(|(name, _)| *name == class)
            .and_then(|(_, values)| values.as_deref())
    }

    pub fn windows(&self) -> impl Iterator<Item = (&'static str, Option<&[i64]>)> {
        self.windows
            .iter()
            .map(|(name, values)| (*name, values.as_deref()))
    }

    pub fn byte(&self, byte: &Byte) -> Option<i64> {
        self.window(byte.class)?.get(byte.index).copied()
    }

    pub fn bit(&self, byte: &Byte, mask: i64) -> Option<bool> {
        self.byte(byte).map(|value| value & mask != 0)
    }

    pub fn software_has_buttons(&self) -> Option<bool> {
        self.byte(&map::HANDOVER).map(|value| value == 1)
    }

    /// A zero is not a stopped fan but the register read mid-update: zeros
    /// arrive as single samples between steady non-zero ones.
    pub fn fan(&self) -> Option<i64> {
        self.byte(&map::FAN).filter(|value| *value != 0)
    }

    /// Temperature points paired with the fan value the controller holds for
    /// each.
    pub fn curve(&self, class: &str) -> Vec<(i64, i64)> {
        let Some(window) = self.window(class) else {
            return Vec::new();
        };
        let temperatures = window.get(map::CURVE_TEMPERATURES).unwrap_or_default();
        let values = window.get(map::CURVE_VALUES).unwrap_or_default();
        temperatures
            .iter()
            .copied()
            .zip(values.iter().copied())
            .collect()
    }

    /// The controller's firmware version: 28 bytes of ASCII holding the name,
    /// then the build date and time run together.
    pub fn firmware(&self) -> Option<String> {
        let window = self.window(map::HANDOVER.class)?;
        let text: String = window
            .get(map::FIRMWARE_RANGE)?
            .iter()
            .filter_map(|value| u8::try_from(*value).ok())
            .filter(u8::is_ascii_graphic)
            .map(char::from)
            .collect();

        match text.len() {
            0 => None,
            28 => Some(format!(
                "{} · {} · {}",
                &text[..12],
                &text[12..20],
                &text[20..]
            )),
            _ => Some(text),
        }
    }
}

/// The windows are declared `UInt8` in the MOF but the batteries are wider and
/// the battery current is signed, so the integer kinds are flattened into the
/// widest signed form rather than assumed.
fn as_i64(value: &Variant) -> Option<i64> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Ec {
        Ec::from_windows(vec![
            (
                "MSI_Software",
                Some(
                    [1, 3, 0, 0, 0, 0]
                        .into_iter()
                        .chain("1796EMS1.1040223201613:46:38".bytes().map(i64::from))
                        .collect(),
                ),
            ),
            ("MSI_AP", Some(vec![0, 20, 137, 0, 0, 0, 44, 0])),
            ("MSI_Device", Some(vec![0x49, 0x4B, 128, 192])),
            (
                "MSI_CPU",
                Some(vec![
                    13, 49, 0, 100, 0, 60, 65, 70, 75, 75, 75, 0, 50, 56, 66, 76, 76, 76, 0,
                ]),
            ),
            (
                "MSI_VGA",
                Some(vec![
                    13, 0, 0, 100, 0, 55, 60, 65, 70, 75, 75, 0, 50, 56, 66, 76, 86, 86,
                ]),
            ),
            (
                "MSI_Master_Battery",
                Some(vec![
                    5, 3924, 10800, 392, 192, 2200, 44, 3561, -2220, 1540, 10604, 3062, 0, 10, 5,
                    12300,
                ]),
            ),
        ])
    }

    #[test]
    fn reads_named_bytes() {
        let ec = sample();
        assert_eq!(ec.software_has_buttons(), Some(true));
        assert_eq!(ec.byte(&map::CPU_TEMPERATURE), Some(49));
        assert_eq!(ec.byte(&map::GPU_TEMPERATURE), Some(0));
        assert_eq!(ec.fan(), Some(137));
    }

    #[test]
    fn keeps_the_negative_current() {
        // The bug this guards against: dropping a negative value shifted every
        // later index in the window, so the voltage read as a temperature.
        let ec = sample();
        assert_eq!(ec.byte(&map::battery::CURRENT), Some(-2220));
        assert_eq!(ec.byte(&map::battery::VOLTAGE), Some(10604));
        assert_eq!(ec.byte(&map::battery::TEMPERATURE), Some(3062));
    }

    #[test]
    fn a_zero_fan_reading_is_treated_as_no_reading() {
        let ec = Ec::from_windows(vec![("MSI_AP", Some(vec![0, 20, 0, 0, 0, 0, 44, 0]))]);
        assert_eq!(ec.fan(), None);
    }

    #[test]
    fn decodes_the_firmware_string() {
        assert_eq!(
            sample().firmware().as_deref(),
            Some("1796EMS1.104 · 02232016 · 13:46:38")
        );
    }

    #[test]
    fn pairs_the_curve() {
        assert_eq!(
            sample().curve("MSI_CPU"),
            vec![(60, 50), (65, 56), (70, 66), (75, 76), (75, 76), (75, 76)]
        );
    }

    #[test]
    fn missing_windows_are_not_fatal() {
        let ec = Ec::default();
        assert_eq!(ec.software_has_buttons(), None);
        assert_eq!(ec.firmware(), None);
        assert_eq!(ec.curve("MSI_CPU"), Vec::new());
    }
}
