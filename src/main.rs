// A windowed program: without this, Windows gives every run a console window.
// The `--console` mode borrows the launching terminal instead, see
// `attach_parent_console`.
#![windows_subsystem = "windows"]

//! Handler for the hardware buttons of an MSI GL72 6QD.
//!
//! It replaces MSI's software completely: no SCM.exe, no `Micro Star SCM`
//! service, no ring-0 driver. What it needs is elevation, because root\WMI is
//! not readable otherwise.
//!
//! Three jobs. It arms the event channel, which is the whole trick of this
//! project — see `ec::set_arming`. It subscribes to the ACPI-WMI event the
//! buttons raise. And it runs whatever the user has mapped to a button.
//!
//! Started with no arguments it opens its window and sits in the notification
//! area; `--console` runs the handler alone, in the foreground, with no window.

mod actions;
mod ec;
mod shared;
mod tray;
mod ui;

use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::thread;
use std::time::Duration;

use anyhow::Result;

use shared::{Press, Shared};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let asked = |name: &str| args.iter().any(|arg| arg == name);

    // Two handlers would both arm the channel and both run whatever is mapped to
    // a pressed button, so the second start hands over to the first instead.
    // The handle is deliberately never closed: the name must stay taken for as
    // long as this process runs, and Windows releases it when the process ends.
    if claim_single_instance().is_none() {
        surface_existing_window();
        return Ok(());
    }

    if asked("--console") {
        attach_parent_console();
        return run_headless();
    }

    let shared = shared::new_shared();
    spawn_workers(&shared);
    run_window(shared)
}

/// Arms the channel, then handles presses until the process ends. Used by
/// `--console` and by the worker thread behind the window.
fn listen(shared: Shared) -> Result<()> {
    let wmi = ec::connect()?;

    match ec::set_arming(&wmi, 1) {
        Ok(true) => note(&shared, None, "канал вооружён"),
        Ok(false) => note(&shared, None, "канал уже был вооружён"),
        // Worth continuing: the channel may have been armed earlier in this
        // boot, in which case events arrive anyway.
        Err(e) => note(&shared, Some(format!("не удалось вооружить канал: {e:#}")), ""),
    }

    if let Err(e) = actions::write_template_if_missing() {
        eprintln!("could not create actions.txt: {e:#}");
    }

    // A notification query blocks until an indication arrives, so this loop
    // costs nothing while idle — no polling, no timer.
    for indication in wmi.notification::<ec::MsiEvent>()? {
        match indication {
            Ok(event) => handle(&shared, event.code),
            Err(e) => eprintln!("subscription error: {e:#}"),
        }
    }

    Ok(())
}

/// Everything that happens on a button press.
fn handle(shared: &Shared, code: u32) {
    let name = shared::button_name(code).unwrap_or("неопознанная");
    println!("0x{code:06X}  {name}");

    let press = {
        let mut state = shared.lock().unwrap();
        let press = Press {
            at: chrono::Local::now().format("%H:%M:%S").to_string(),
            code,
            cpu: state.snapshot.cpu,
            fan: state.snapshot.fan,
            on_mains: state.snapshot.on_mains,
        };
        state.remember(press.clone());

        // While the assign dialog is listening, a press picks the button
        // instead of firing whatever is mapped to it.
        if state.listening {
            state.listening = false;
            state.caught = Some(code);
            append_log(&press, Some("выбрана в диалоге"));
            return;
        }
        press
    };

    match actions::load().get(&code) {
        Some(command) if !command.is_empty() => match actions::run(command) {
            Ok(()) => append_log(&press, Some(&format!("запущено: {command}"))),
            Err(e) => append_log(&press, Some(&format!("НЕ запустилось {command}: {e:#}"))),
        },
        _ => append_log(&press, None),
    }
}

/// One line per press in `events.log`. A failure here must not take the handler
/// down, so it is reported and swallowed.
fn append_log(press: &Press, extra: Option<&str>) {
    let name = shared::button_name(press.code).unwrap_or("неопознанная");
    let mut line = format!(
        "{}  0x{:06X}  {name:28}  CPU {}  вентилятор {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        press.code,
        press.cpu.map_or("—".to_string(), |t| format!("{t} °C")),
        press.fan.map_or("—".to_string(), |v| v.to_string()),
    );
    if let Some(extra) = extra {
        line.push_str("  ");
        line.push_str(extra);
    }
    line.push('\n');

    let path = actions::log_path();
    if let Some(dir) = path.parent() {
        let _ = create_dir_all(dir);
    }
    let written = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()));
    if let Err(e) = written {
        eprintln!("cannot write {}: {e}", path.display());
    }
}

fn note(shared: &Shared, error: Option<String>, message: &str) {
    if !message.is_empty() {
        println!("{message}");
    }
    shared.lock().unwrap().note = error;
}

/// Two threads, because each needs its own WMI connection: the subscription
/// blocks, and the readings are polled.
fn spawn_workers(shared: &Shared) {
    let listener = shared.clone();
    thread::spawn(move || {
        if let Err(e) = listen(listener.clone()) {
            note(&listener, Some(format!("{e:#}")), "");
            eprintln!("handler stopped: {e:#}");
        }
    });

    let poller = shared.clone();
    thread::spawn(move || match ec::connect() {
        Ok(wmi) => loop {
            let snapshot = ec::snapshot(&wmi);
            poller.lock().unwrap().snapshot = snapshot;
            thread::sleep(Duration::from_secs(1));
        },
        Err(e) => {
            note(&poller, Some(format!("{e:#}")), "");
        }
    });
}

fn run_headless() -> Result<()> {
    let shared = shared::new_shared();
    let poller = shared.clone();
    thread::spawn(move || {
        if let Ok(wmi) = ec::connect() {
            loop {
                poller.lock().unwrap().snapshot = ec::snapshot(&wmi);
                thread::sleep(Duration::from_secs(1));
            }
        }
    });
    listen(shared)
}

fn run_window(shared: Shared) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size(ui::WINDOW)
            .with_min_inner_size(ui::WINDOW)
            .with_resizable(false)
            // The mockup draws its own 30px title bar.
            .with_decorations(false)
            .with_title("msi-hotkeys"),
        ..Default::default()
    };

    eframe::run_native(
        "msi-hotkeys",
        options,
        Box::new(move |cc| {
            // The tray icon has to be made on the thread that owns the event
            // loop, which is this one.
            let (tray, menu) = match tray::build() {
                Ok((tray, menu)) => (Some(tray), menu),
                Err(e) => {
                    eprintln!("no tray icon: {e:#}");
                    (None, tray::Menu::none())
                }
            };
            Ok(Box::new(ui::App::new(&cc.egui_ctx, shared, tray, menu)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Hands the buttons back to the firmware on the way out.
pub fn leave() {
    match ec::set_armed_standalone(false) {
        Ok(true) => println!("канал разоружён"),
        Ok(false) => println!("канал и так не был вооружён"),
        Err(e) => eprintln!("не удалось снять вооружение: {e:#}"),
    }
}

/// Takes the name that marks "a handler is running", or `None` if someone else
/// holds it.
///
/// The name is session-local, so it also catches the case worth catching here:
/// a second copy started without elevation, which cannot read root\WMI and would
/// sit there showing nothing. An elevated holder labels the mutex at its own
/// integrity level, so the unelevated attempt is refused outright rather than
/// told the name is taken — either answer means the same thing.
fn claim_single_instance() -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    unsafe {
        let handle = CreateMutexW(None, true, w!("Local\\msi-hotkeys-single-instance")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return None;
        }
        Some(handle)
    }
}

/// Brings the running handler's window forward, so starting the program again
/// does what the person meant by it.
fn surface_existing_window() {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    unsafe {
        if let Ok(window) = FindWindowW(None, w!("msi-hotkeys")) {
            let _ = ShowWindow(window, SW_RESTORE);
            let _ = SetForegroundWindow(window);
        }
    }
}

/// A windows-subsystem program starts with no console, so `--console` borrows
/// the one it was launched from and points the standard handles at it. Without
/// re-opening CONOUT$ the handles stay invalid and nothing is printed.
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
