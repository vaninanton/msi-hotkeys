//! State the worker threads write and the interface reads.

use std::sync::{Arc, Mutex};

use crate::ec::Snapshot;

/// Every button this machine has been seen to raise, in the order the interface
/// lists them. The ones with a name are identified; the rest are real buttons
/// whose purpose is not known, and saying so is more useful than inventing one.
///
/// Three names come from the scancode table in Linux' `msi-wmi.c`, which
/// annotates the Fn key each one belongs to. The low byte of our code is that
/// scancode: `0x08` is `WIND_KEY_TOUCHPAD` ("Fn+F3 touchpad toggle") and `0x57`
/// is `WIND_KEY_CAMERA` ("Fn+F6 webcam toggle"). The camera appears twice,
/// differing only in the middle byte — the pattern the project noticed before
/// knowing what it meant, and most likely on against off.
pub const BUTTONS: [(u32, Option<&str>); 13] = [
    (0x22_0004, Some("Cooler Boost")),
    (0x22_0029, Some("Свободная, рядом с питанием")),
    (0x22_0021, Some("Громкость −")),
    (0x22_0032, Some("Громкость +")),
    (0x22_0208, Some("Тачпад · Fn+F3")),
    (0x22_4957, Some("Камера · Fn+F6")),
    (0x22_4B57, Some("Камера · Fn+F6")),
    (0x22_0023, None),
    (0x22_0062, None),
    (0x22_0063, None),
    (0x22_0079, None),
    (0x22_0279, None),
    (0x22_006F, None),
];

pub fn button_name(code: u32) -> Option<&'static str> {
    BUTTONS
        .iter()
        .find(|(known, _)| *known == code)
        .and_then(|(_, name)| *name)
}

/// One keypress, as the log and the interface show it.
#[derive(Clone, Debug)]
pub struct Press {
    pub at: String,
    pub code: u32,
    pub cpu: Option<u64>,
    pub fan: Option<u64>,
    pub on_mains: Option<bool>,
}

#[derive(Default)]
pub struct State {
    pub snapshot: Snapshot,
    /// Newest first, capped — the full history lives in the log file.
    pub presses: Vec<Press>,
    /// Last thing worth telling the user, shown in the status bar.
    pub note: Option<String>,
    /// Set by the assign dialog: the next press is taken as the selection
    /// rather than acted on.
    pub listening: bool,
    /// Where `listening` puts what it caught.
    pub caught: Option<u32>,
}

pub type Shared = Arc<Mutex<State>>;

pub fn new_shared() -> Shared {
    Arc::new(Mutex::new(State::default()))
}

impl State {
    pub fn remember(&mut self, press: Press) {
        self.presses.insert(0, press);
        self.presses.truncate(200);
    }
}
