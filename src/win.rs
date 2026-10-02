//! The Windows calls. Every `unsafe` block in the program is in this file, so
//! reviewing the platform glue means reviewing one module.

/// Takes the name that marks "a handler is running", reporting whether it was
/// free. The handle is deliberately never closed: the name must stay taken for
/// as long as this process runs, and Windows releases it when the process ends.
pub fn claim_single_instance() -> bool {
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
pub fn attach_parent_console() {
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

/// Hands the buttons back when the console window is closed or Ctrl+C pressed,
/// then leaves.
/// Without it Ctrl+C would leave the buttons with this process, which is
/// survivable but untidy: the firmware should get them back.
pub fn release_buttons_on_console_exit() {
    use windows::core::BOOL;
    use windows::Win32::System::Console::SetConsoleCtrlHandler;

    unsafe extern "system" fn on_exit_signal(_kind: u32) -> BOOL {
        crate::handler::release_buttons();
        std::process::exit(0);
    }

    unsafe {
        let _ = SetConsoleCtrlHandler(Some(on_exit_signal), true);
    }
}

/// Turns on escape-sequence handling for this console, reporting whether it
/// worked. A redirected or very old console will refuse, which is why the caller
/// keeps a fallback.
pub fn enable_virtual_terminal() -> bool {
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
pub fn pump_messages() {
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

/// Shows a toast. An unpackaged Win32 program has no `AppUserModelID` of its own
/// and a toast sent without one is silently dropped, so PowerShell's registered
/// id is borrowed — the usual way around that.
pub fn notify(title: &str, body: &str) {
    use tauri_winrt_notification::Toast;

    if let Err(e) = Toast::new(Toast::POWERSHELL_APP_ID)
        .title(title)
        .text1(body)
        .show()
    {
        eprintln!("toast failed: {e}");
    }
}
