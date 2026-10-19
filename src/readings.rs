//! Turning a reading into text. No WMI here, which is what makes it testable.
//!
//! The lines the user sees are in Russian; the console and the log stay in
//! English.

use std::fmt::Write as _;

use crate::ec::{map, Ec, Power};

/// How many lines the tray menu shows. Fixed, because a menu that changes shape
/// depending on what answered is harder to read than one with a dash in it.
pub const LINES: usize = 11;

/// The readings as separate lines, one value each.
pub fn lines(ec: &Ec, power: Option<&Power>) -> [String; LINES] {
    let [buttons, cpu, gpu, fan, mode, devices] = machine_lines(ec);
    let [source, charge, battery, capacity, state] = power_lines(ec, power);
    [
        buttons, cpu, gpu, fan, mode, devices, source, charge, battery, capacity, state,
    ]
}

fn dash(label: &str) -> String {
    format!("{label}: —")
}

const fn on_off(on: bool) -> &'static str {
    if on {
        "вкл"
    } else {
        "выкл"
    }
}

/// The six lines about the machine itself.
fn machine_lines(ec: &Ec) -> [String; 6] {
    [
        match ec.software_has_buttons() {
            Some(true) => "Кнопки: у программы".to_string(),
            Some(false) => "Кнопки: у прошивки".to_string(),
            None => dash("Кнопки"),
        },
        ec.byte(&map::CPU_TEMPERATURE)
            .map_or_else(|| dash("CPU"), |value| format!("CPU: {value} °C")),
        // The discrete card is powered down at idle, and then its temperature
        // reads zero rather than being absent.
        match ec.byte(&map::GPU_TEMPERATURE) {
            Some(0) => "GPU: спит".to_string(),
            Some(value) => format!("GPU: {value} °C"),
            None => dash("GPU"),
        },
        fan_line(ec),
        // Both bits live in EC 0xE4 and both come with a power-LED colour in
        // msi-laptop: orange for Turbo, green for ECO.
        match (
            ec.bit(&map::TURBO, map::TURBO_BIT),
            ec.bit(&map::FUNCTIONS_HIGH, map::ECO_BIT),
        ) {
            (Some(true), _) => "Профиль: Turbo".to_string(),
            (_, Some(true)) => "Профиль: ECO".to_string(),
            (Some(false), Some(false)) => "Профиль: обычный".to_string(),
            _ => dash("Профиль"),
        },
        devices_line(ec),
    ]
}

/// The measured fan speed, raw: ~75 at idle, ~135 under Cooler Boost, and no
/// unit to put after it. Nothing else is claimed, because nothing else is known.
///
/// Two earlier versions claimed more. One labelled this number with the `0x33`
/// bit as "on the curve / flat out" — that bit is the controller's own thermal
/// override, untouched by Cooler Boost. The next showed EC `0x71` as a duty with
/// "Cooler Boost" at 86, on the strength of press-time snapshots; a continuous
/// recording then showed `0x71` sitting at 0 through a whole boost while this
/// tachometer climbed, and no other byte in any window moving at all. So the
/// boost state is not readable here, and the line says only what is.
fn fan_line(ec: &Ec) -> String {
    ec.fan_tach()
        .map_or_else(|| dash("Вентилятор"), |tach| format!("Вентилятор: {tach}"))
}

/// Only the parts the machine actually has, each with its state. Worth showing
/// because these bits are the only indication the machine gives that the camera
/// or the touchpad has been switched off underneath Windows.
fn devices_line(ec: &Ec) -> String {
    let mut listed: Vec<String> = Vec::new();

    if let (Some(state), Some(present)) =
        (ec.byte(&map::DEVICE_STATE), ec.byte(&map::DEVICE_PRESENT))
    {
        listed.extend(
            map::DEVICES
                .iter()
                .filter(|(mask, _)| present & mask != 0)
                .map(|(mask, name)| format!("{name} {}", on_off(state & mask != 0))),
        );
    }

    // The touchpad sits in a byte of its own, with no presence bit beside it, so
    // it is read separately rather than through the mask table.
    if let Some(on) = ec.bit(&map::TOUCHPAD, map::TOUCHPAD_ON) {
        listed.push(format!("тачпад {}", on_off(on)));
    }

    if listed.is_empty() {
        dash("Устройства")
    } else {
        format!("Устройства: {}", listed.join(" · "))
    }
}

/// The five lines about power. They come from the controller, with the Windows
/// figures as a fallback where it has one.
fn power_lines(ec: &Ec, power: Option<&Power>) -> [String; 5] {
    use map::battery as bat;

    [
        power.map_or_else(
            || dash("Питание"),
            |power| {
                let source = if power.on_mains {
                    "от сети"
                } else {
                    "от батареи"
                };
                format!("Питание: {source}")
            },
        ),
        // Percentage and current come from the controller; Windows agrees to
        // within a point, and the current carries the sign that says which way
        // the charge is going.
        match (
            ec.byte(&bat::PERCENT)
                .or_else(|| power.and_then(|p| p.percent).map(i64::from)),
            ec.byte(&bat::CURRENT),
        ) {
            (Some(percent), Some(current)) if current != 0 => {
                format!("Заряд: {percent} % · {current} мА")
            }
            (Some(percent), _) => format!("Заряд: {percent} %"),
            _ => dash("Заряд"),
        },
        // Straight from the controller, where the kernel's own field names say
        // what these are: the voltage in millivolts and the temperature in
        // tenths of a kelvin.
        match (ec.byte(&bat::VOLTAGE), ec.byte(&bat::TEMPERATURE)) {
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
        match (ec.byte(&bat::LEFT), ec.byte(&bat::FULL)) {
            (Some(left), Some(full)) if full > 0 => format!("Ёмкость: {left} из {full} мА·ч"),
            _ => dash("Ёмкость"),
        },
        battery_state_line(ec),
    ]
}

fn battery_state_line(ec: &Ec) -> String {
    use map::battery as bat;

    let Some(flags) = ec.byte(&bat::FLAGS) else {
        return dash("Состояние");
    };

    let named = [
        (bat::PRESENT, "установлена"),
        (bat::CHARGING, "заряжается"),
        (bat::DISCHARGING, "разряжается"),
        (bat::FULL_FLAG, "заряжена"),
    ];
    let state: Vec<&str> = named
        .iter()
        .filter(|(mask, _)| flags & mask != 0)
        .map(|(_, name)| *name)
        .collect();

    if state.is_empty() {
        dash("Состояние")
    } else {
        format!("Состояние: {}", state.join(" · "))
    }
}

/// EC words are 16-bit, so the conversions go through `f64` without a narrowing
/// cast: a reading wider than expected should look wrong, not silently wrap.
fn millivolts(value: i64) -> f64 {
    f64::from(i32::try_from(value).unwrap_or(i32::MAX)) / 1000.0
}

fn kelvin_tenths(value: i64) -> f64 {
    f64::from(i32::try_from(value).unwrap_or(i32::MAX)) / 10.0 - 273.15
}

/// The machine in one line, for the tooltip and the log.
pub fn summary(ec: &Ec, power: Option<&Power>) -> String {
    let mut facts = Vec::new();

    if let Some(value) = ec.byte(&map::CPU_TEMPERATURE) {
        facts.push(format!("CPU ~{value}C"));
    }

    // Raw, as the menu shows it: the unit is unknown and there is no boost state
    // to read, so a threshold here would be an interpretation.
    facts.push(match ec.fan_tach() {
        Some(tach) => format!("fan {tach}"),
        None => "fan —".to_string(),
    });

    if let Some(power) = power {
        facts.push(
            if power.on_mains {
                "on mains"
            } else {
                "on battery"
            }
            .to_string(),
        );
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

/// Every byte raw, for correlating a press with whatever the controller changed
/// at that moment. This dump is what identifying the remaining buttons and bytes
/// will be done from.
pub fn dump(ec: &Ec) -> String {
    let mut out = String::new();

    for (class, values) in ec.windows() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ec() -> Ec {
        Ec::from_windows(vec![
            ("MSI_Software", Some(vec![1])),
            ("MSI_AP", Some(vec![0, 20, 137])),
            // Bluetooth, camera and WLAN present, the camera switched off.
            ("MSI_Device", Some(vec![0x49, 0x4B, 0x80])),
            // [0] bit 1 set: Turbo. [1] bit 0 clear: not ECO.
            ("MSI_System", Some(vec![0b010, 0b10])),
            // Temperature 52, duty 50, ceiling 100, then the curve.
            (
                "MSI_CPU",
                Some(vec![
                    13, 52, 50, 100, 0, 60, 65, 70, 75, 75, 75, 0, 50, 56, 66, 76, 76, 76,
                ]),
            ),
            // Card asleep: temperature and fan both zero.
            ("MSI_VGA", Some(vec![13, 0, 0, 100])),
            (
                "MSI_Master_Battery",
                Some(vec![5, 0, 0, 0, 0, 0, 58, 3532, -1952, 2040, 11420, 3092]),
            ),
        ])
    }

    fn on_battery() -> Power {
        Power {
            on_mains: false,
            charging: false,
            discharging: true,
            volts: 11.42,
            percent: Some(58),
        }
    }

    #[test]
    fn every_line_is_filled() {
        let lines = lines(&ec(), Some(&on_battery()));
        assert_eq!(lines.len(), LINES);
        for line in &lines {
            assert!(!line.ends_with('—'), "unfilled line: {line}");
        }
    }

    #[test]
    fn reports_the_machine() {
        let [buttons, cpu, gpu, fan, mode, devices] = machine_lines(&ec());
        assert_eq!(buttons, "Кнопки: у программы");
        assert_eq!(cpu, "CPU: 52 °C");
        assert_eq!(gpu, "GPU: спит");
        assert_eq!(fan, "Вентилятор: 137");
        assert_eq!(mode, "Профиль: Turbo");
        assert_eq!(
            devices,
            "Устройства: Bluetooth вкл · камера выкл · WLAN вкл · тачпад вкл"
        );
    }

    #[test]
    fn the_touchpad_is_read_from_its_own_byte() {
        // No presence bit sits beside it, so it is listed whenever it reads.
        let ec = Ec::from_windows(vec![("MSI_Device", Some(vec![0x49, 0x4B, 0x00]))]);
        assert_eq!(
            devices_line(&ec),
            "Устройства: Bluetooth вкл · камера выкл · WLAN вкл · тачпад выкл"
        );

        // And the other way round: the devices bytes missing must not hide it.
        let ec = Ec::from_windows(vec![("MSI_Device", Some(vec![]))]);
        assert_eq!(devices_line(&ec), "Устройства: —");
    }

    #[test]
    fn reports_power() {
        let [source, charge, battery, capacity, state] = power_lines(&ec(), Some(&on_battery()));
        assert_eq!(source, "Питание: от батареи");
        assert_eq!(charge, "Заряд: 58 % · -1952 мА");
        assert_eq!(battery, "Батарея: 11.42 В · 36.1 °C");
        assert_eq!(capacity, "Ёмкость: 2040 из 3532 мА·ч");
        assert_eq!(state, "Состояние: установлена · разряжается");
    }

    #[test]
    fn the_fan_line_claims_nothing_beyond_the_tachometer() {
        // 0x71 at 86 and 0x33 cleared were both once taken for boost; neither
        // may leak into the line now, whatever they read.
        let ec = Ec::from_windows(vec![
            ("MSI_AP", Some(vec![0, 20, 134])),
            ("MSI_CPU", Some(vec![0x00, 71, 86, 100])),
        ]);
        assert_eq!(fan_line(&ec), "Вентилятор: 134");

        // A mid-update zero in the tachometer is no reading, not a stopped fan.
        let ec = Ec::from_windows(vec![("MSI_AP", Some(vec![0, 20, 0]))]);
        assert_eq!(fan_line(&ec), "Вентилятор: —");
    }

    #[test]
    fn eco_shows_when_its_bit_is_set() {
        let ec = Ec::from_windows(vec![("MSI_System", Some(vec![0b000, 0b01]))]);
        let [_, _, _, _, mode, _] = machine_lines(&ec);
        assert_eq!(mode, "Профиль: ECO");
    }

    #[test]
    fn an_empty_reading_gives_dashes_rather_than_a_shorter_menu() {
        let lines = lines(&Ec::default(), None);
        assert_eq!(lines.len(), LINES);
        for line in &lines {
            assert!(line.ends_with('—'), "should be a dash: {line}");
        }
    }

    #[test]
    fn falls_back_to_the_windows_figures() {
        let [_, charge, battery, _, _] = power_lines(&Ec::default(), Some(&on_battery()));
        assert_eq!(charge, "Заряд: 58 %");
        assert_eq!(battery, "Батарея: 11.42 В");
    }

    #[test]
    fn summarises_in_one_line() {
        assert_eq!(
            summary(&ec(), Some(&on_battery())),
            "CPU ~52C  fan 137  on battery  discharging  11.42V  58%"
        );
        assert_eq!(summary(&Ec::default(), None), "fan —");
    }

    #[test]
    fn dumps_unreadable_windows_as_such() {
        let ec = Ec::from_windows(vec![("MSI_CPU", Some(vec![1, 255])), ("MSI_VGA", None)]);
        assert_eq!(dump(&ec), "MSI_CPU=[01 FF]  MSI_VGA=<unreadable>");
    }
}
