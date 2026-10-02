//! The buttons this machine raises events for.
//!
//! One table rather than scattered constants: an earlier version kept these in
//! two places and the two drifted apart.
//!
//! The brightness pair is named from the owner's own presses rather than from a
//! source. Like the volume keys, both are left alone: the firmware has already
//! acted by the time the event arrives.
//!
//! The codes are not all of the form `0x2200NN` — `0x220208`, `0x224957` and
//! `0x224B57` carry something in the middle byte too. The names come from the
//! scancode table in Linux' `msi-wmi.c`, which is keyed by our low byte: `0x08`
//! is `WIND_KEY_TOUCHPAD` ("Fn+F3 touchpad toggle") and `0x57` is
//! `WIND_KEY_CAMERA` ("Fn+F6 webcam toggle"). The camera appears under two codes
//! differing only in that middle byte, most likely on against off.

/// Every button seen to raise an event, with the name where there is one.
pub const ALL: [(u32, &str); 9] = [
    (0x22_0004, "Cooler Boost"),
    (0x22_0029, "Center key"), // "MSI M-Center main menu" in the kernel
    (0x22_0021, "Volume down"),
    (0x22_0032, "Volume up"),
    (0x22_0062, "Brightness up"),
    (0x22_0063, "Brightness down"),
    (0x22_0208, "Touchpad (Fn+F3)"),
    (0x22_4957, "Camera (Fn+F6)"),
    (0x22_4B57, "Camera (Fn+F6)"),
];

/// The key next to the power button. It does nothing in hardware, which makes it
/// the one free button here, so it gets the fallback behaviour.
pub const CENTER: u32 = 0x22_0029;

pub fn name(code: u32) -> &'static str {
    ALL.iter()
        .find(|(known, _)| *known == code)
        .map_or("unidentified", |(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_known_codes_and_admits_the_rest() {
        assert_eq!(name(0x22_0004), "Cooler Boost");
        assert_eq!(name(CENTER), "Center key");
        assert_eq!(name(0x00_0001), "unidentified");
    }

    #[test]
    fn the_centre_key_is_in_the_table() {
        assert!(ALL.iter().any(|(code, _)| *code == CENTER));
    }
}
