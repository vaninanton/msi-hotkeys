//! The notification-area icon and its menu.
//!
//! The icon lives on the thread that runs the window's event loop, because that
//! is the thread whose message queue it is created on; `eframe` pumps that queue
//! for us, and `Menu::poll` picks the clicks out.

use anyhow::{anyhow, Result};
use tray_icon::menu::{Menu as TrayMenu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub enum Action {
    Open,
    OpenLog,
    Quit,
}

/// The ids of the items, so a click can be told apart from the others.
pub struct Menu {
    open: MenuId,
    log: MenuId,
    quit: MenuId,
}

impl Menu {
    pub fn poll(&self) -> Option<Action> {
        let event = MenuEvent::receiver().try_recv().ok()?;
        if event.id == self.open {
            Some(Action::Open)
        } else if event.id == self.log {
            Some(Action::OpenLog)
        } else if event.id == self.quit {
            Some(Action::Quit)
        } else {
            None
        }
    }

    /// A placeholder for when the tray could not be created: polling it never
    /// yields anything, and the window still works.
    pub fn none() -> Self {
        Self {
            open: MenuId::new("open"),
            log: MenuId::new("log"),
            quit: MenuId::new("quit"),
        }
    }
}

pub fn build() -> Result<(TrayIcon, Menu)> {
    let open = MenuItem::new("Открыть окно", true, None);
    let log = MenuItem::new("Открыть журнал", true, None);
    let quit = MenuItem::new("Снять вооружение и выйти", true, None);

    let menu = TrayMenu::new();
    menu.append_items(&[
        &open,
        &log,
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    let ids = Menu {
        open: open.id().clone(),
        log: log.id().clone(),
        quit: quit.id().clone(),
    };

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(image()?)
        .with_tooltip("msi-hotkeys")
        .build()?;

    Ok((tray, ids))
}

/// Drawn rather than shipped: a disc in the accent colour with a dark hub,
/// which reads at 16 pixels and needs no asset beside the binary.
fn image() -> Result<Icon> {
    const SIZE: u32 = 32;
    let centre = (SIZE as f32 - 1.0) / 2.0;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);

    for y in 0..SIZE {
        for x in 0..SIZE {
            let radius = (x as f32 - centre).hypot(y as f32 - centre);
            let pixel = if radius > centre {
                [0, 0, 0, 0]
            } else if radius < centre * 0.3 {
                [0x16, 0x18, 0x26, 255]
            } else {
                [0x91, 0x84, 0xd9, 255]
            };
            rgba.extend_from_slice(&pixel);
        }
    }

    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|e| anyhow!("could not build the icon: {e}"))
}
