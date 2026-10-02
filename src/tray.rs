//! The notification-area icon: live readings in the menu, and a quit that hands
//! the buttons back on the way out.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::ec::Machine;
use crate::{handler, readings, win};

/// How often the readings are taken.
const INTERVAL: Duration = Duration::from_secs(1);

/// Runs until the user picks Выход.
///
/// Three threads, for reasons that are not stylistic. The subscription blocks,
/// so it cannot share a thread with anything. The readings cannot be taken on
/// this thread either: the tray initialises COM here as a single-threaded
/// apartment, and a WMI connection made afterwards appears to open while every
/// query returns nothing. So this thread does nothing but pump messages and copy
/// strings.
pub fn run() -> Result<()> {
    std::thread::spawn(|| {
        if let Err(e) = handler::listen() {
            eprintln!("handler stopped: {e:#}");
        }
    });

    let published = spawn_poller();

    // Disabled items: a native menu has no other way to show a line of text, and
    // greyed out is also the honest look for something that is a reading rather
    // than a command.
    let lines: Vec<MenuItem> = (0..readings::LINES)
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
        .with_icon(icon()?)
        .with_tooltip("msi-hotkeys")
        .build()?;

    let mut shown = Published::default();

    loop {
        win::pump_messages();

        // Copying strings, so this can run often; the cost is on the polling
        // thread, which sets its own pace. A poisoned lock would mean the poller
        // died mid-write: keep showing the last good readings rather than taking
        // the tray down with it.
        let latest = match published.lock() {
            Ok(published) => published.clone(),
            Err(_) => shown.clone(),
        };
        if latest != shown {
            for (item, text) in lines.iter().zip(&latest.lines) {
                item.set_text(text);
            }
            let _ = tray.set_tooltip(Some(format!("msi-hotkeys — {}", latest.tooltip)));
            shown = latest;
        }

        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == *quit.id() {
                handler::release_buttons();
                return Ok(());
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What the polling thread hands to the tray: the menu lines and the tooltip,
/// already formatted. The menu never touches WMI.
#[derive(Clone, Default, PartialEq, Eq)]
struct Published {
    lines: Vec<String>,
    tooltip: String,
}

/// Reads the machine on a thread of its own and publishes the formatted result.
fn spawn_poller() -> Arc<Mutex<Published>> {
    let published = Arc::new(Mutex::new(Published {
        lines: vec!["…".to_string(); readings::LINES],
        tooltip: "читаю…".to_string(),
    }));

    let writer = Arc::clone(&published);
    std::thread::spawn(move || {
        let machine = match Machine::open() {
            Ok(machine) => machine,
            Err(e) => {
                // The menu is the only place this can be reported: there is no
                // console in tray mode.
                if let Ok(mut published) = writer.lock() {
                    published.lines = vec![format!("{e:#}")];
                    published.tooltip = format!("{e:#}");
                }
                return;
            }
        };

        loop {
            let ec = machine.read();
            let power = machine.power();
            let next = Published {
                lines: readings::lines(&ec, power.as_ref()).to_vec(),
                tooltip: readings::summary(&ec, power.as_ref()),
            };
            match writer.lock() {
                Ok(mut published) => *published = next,
                // The tray thread is gone, so there is nobody to publish to.
                Err(_) => return,
            }
            std::thread::sleep(INTERVAL);
        }
    });

    published
}

/// The icon, drawn rather than shipped: a disc with a dark hub, which reads at
/// 16 pixels and needs no asset file next to the binary.
fn icon() -> Result<Icon> {
    const SIZE: u32 = 32;
    const RIM: [u8; 4] = [86, 184, 214, 255];
    const HUB: [u8; 4] = [24, 28, 34, 255];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    let centre = f64::from(SIZE - 1) / 2.0;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);

    for y in 0..SIZE {
        for x in 0..SIZE {
            let radius = (f64::from(x) - centre).hypot(f64::from(y) - centre);
            let pixel = if radius > centre {
                CLEAR
            } else if radius < centre * 0.3 {
                HUB
            } else {
                RIM
            };
            rgba.extend_from_slice(&pixel);
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| anyhow!("could not build the icon: {e}"))
}
