//! The buttons this machine raises events for.
//!
//! One table rather than scattered constants: an earlier version kept these in
//! two places and the two drifted apart.
//!
//! The brightness pair is named from the owner's own presses rather than from a
//! source. Like the volume keys, both are left alone: the firmware has already
//! acted by the time the event arrives.
//!
//! The codes are not all of the form `0x2200NN`: some carry something in the
//! middle byte too. The low byte matches the scancode table in Linux'
//! `msi-wmi.c` — `0x08` is `WIND_KEY_TOUCHPAD` ("Fn+F3 touchpad toggle") and
//! `0x57` is `WIND_KEY_CAMERA` ("Fn+F6 webcam toggle").
//!
//! Three keys turn up under a pair of codes each, and in every pair the two
//! differ by exactly bit 1 of that middle byte: `0x220208`/`0x220008` for the
//! touchpad, `0x220279`/`0x220079` for ECO, `0x224B57`/`0x224957` for the camera.
//!
//! For the camera that byte is a snapshot of EC `0x2E`, the device-state byte,
//! confirmed by six presses against the live byte. For the other two it is
//! something else, still unexplained — see docs/FINDINGS.md.

/// Every button seen to raise an event, with the name where there is one. A
/// device that appears under two codes is listed twice rather than masked: the
/// meaning of the differing bit is still a guess, and a mask would bake it in.
pub const ALL: [(u32, &str); 13] = [
    // A key of its own in the row beside the power button, not an Fn combination.
    // The kernel's table has no code ending in 0x04, and the mute keys it does
    // know sit in 0xD0..0xD4 — so nothing suggests this code is shared, but the
    // machine's own mute key (Fn+Num0) has not been checked against it.
    (0x22_0004, "Cooler Boost"),
    // Labelled Gaming Center on this machine, between Cooler Boost and the power
    // button. The kernel calls the same code CLAW_KEY_CENTER, "MSI M-Center main
    // menu".
    (0x22_0029, "Gaming Center"),
    (0x22_0021, "Volume down"),
    (0x22_0032, "Volume up"),
    // Not in the kernel's table at all. MSI's own software treats it as "launch
    // the user's application", and nothing in hardware responds to it, so it is
    // the natural key to map in actions.txt.
    (0x22_006F, "P1 (Fn+F4)"),
    // Marked with a battery and the word ECO. Five presses changed not one EC
    // byte, so the hardware does nothing with it either: MSI's own software
    // switched a Windows power plan. Another key free to map.
    (0x22_0279, "ECO (Fn+F5)"),
    (0x22_0079, "ECO (Fn+F5)"),
    (0x22_0062, "Brightness up"),
    (0x22_0063, "Brightness down"),
    (0x22_0208, "Touchpad (Fn+F3)"),
    (0x22_0008, "Touchpad (Fn+F3)"),
    (0x22_4957, "Camera (Fn+F6)"),
    (0x22_4B57, "Camera (Fn+F6)"),
];

/// The Gaming Center key, between Cooler Boost and the power button. It does
/// nothing in hardware, which makes it a free button, so it gets the fallback
/// behaviour. P1 and ECO are free in the same sense but have no fallback: one key
/// reporting the machine is enough, and P1 is the one the user is expected to map.
pub const GAMING_CENTER: u32 = 0x22_0029;

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
        assert_eq!(name(GAMING_CENTER), "Gaming Center");
        assert_eq!(name(0x00_0001), "unidentified");
    }

    #[test]
    fn the_free_key_is_in_the_table() {
        assert!(ALL.iter().any(|(code, _)| *code == GAMING_CENTER));
    }
}
