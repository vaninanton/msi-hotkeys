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

// --- raw keyboard input, only in the `scan` build ----------------------------

/// One raw input event, decoded far enough to identify a key.
///
/// Kept behind the `scan` feature so that the published binary carries no
/// keyboard-reading code at all. A tray program that asks for administrator
/// rights and reads keystrokes is a keylogger by shape, whatever it was built
/// for, and heuristic scanners are right to say so.
#[cfg(feature = "scan")]
#[derive(Clone, Debug)]
pub enum KeyEvent {
    Keyboard {
        make_code: u16,
        flags: u16,
        vkey: u16,
        message: u32,
    },
    Hid {
        device: String,
        bytes: Vec<u8>,
    },
}

/// A Raw Input subscription on a message-only window.
///
/// `RIDEV_INPUTSINK` is what makes this work without a visible window: input is
/// delivered even though this process never has focus. Reading the HID
/// collections directly was tried first and refused — Windows holds the keyboard
/// ones exclusively — and Raw Input is the supported way in.
#[cfg(feature = "scan")]
#[derive(Debug)]
pub struct RawInput {
    window: windows::Win32::Foundation::HWND,
    names: std::collections::HashMap<isize, String>,
}

#[cfg(feature = "scan")]
impl RawInput {
    /// Registers for keyboard, consumer-control and the vendor collections this
    /// machine exposes. The last two are there because the keys in question may
    /// not arrive as keyboard input at all.
    pub fn open() -> anyhow::Result<Self> {
        use windows::core::w;
        use windows::Win32::UI::Input::{
            RegisterRawInputDevices, RAWINPUTDEVICE, RAWINPUTDEVICE_FLAGS, RIDEV_INPUTSINK,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
        };

        // A message-only window: never shown, never painted, it exists so that
        // raw input has somewhere to be delivered. The built-in STATIC class is
        // used rather than a class of our own, which would need a window
        // procedure and a brush we have no use for.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )?
        };

        let device = |page: u16, usage: u16| RAWINPUTDEVICE {
            usUsagePage: page,
            usUsage: usage,
            dwFlags: RAWINPUTDEVICE_FLAGS(RIDEV_INPUTSINK.0),
            hwndTarget: window,
        };
        let devices = [
            device(0x01, 0x06),   // keyboard
            device(0x01, 0x80),   // system control
            device(0x0C, 0x01),   // consumer control: media and display keys
            device(0xFF00, 0x0E), // the vendor collection this keyboard exposes
        ];

        unsafe {
            RegisterRawInputDevices(
                &devices,
                u32::try_from(size_of::<RAWINPUTDEVICE>()).expect("fits"),
            )?;
        }

        Ok(Self {
            window,
            names: std::collections::HashMap::new(),
        })
    }

    /// Drains whatever has arrived since the last call.
    pub fn poll(&mut self) -> Vec<KeyEvent> {
        use windows::Win32::UI::Input::{
            GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTHEADER, RID_INPUT, RIM_TYPEHID,
            RIM_TYPEKEYBOARD,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            DefWindowProcW, PeekMessageW, MSG, PM_REMOVE, WM_INPUT,
        };

        // Taken from the crate rather than written out: these were once 0 and 2
        // here, and 0 is the mouse, so every keypress went to the discarding arm
        // and a scan that worked perfectly reported nothing.
        const KEYBOARD: u32 = RIM_TYPEKEYBOARD.0;
        const HID: u32 = RIM_TYPEHID.0;

        let mut events = Vec::new();

        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&raw mut message, Some(self.window), 0, 0, PM_REMOVE).as_bool() {
                if message.message != WM_INPUT {
                    continue;
                }

                // Large enough for a keyboard event and for the HID reports this
                // keyboard sends; anything longer is truncated rather than lost.
                let mut buffer = [0_u8; 1024];
                let mut size = u32::try_from(buffer.len()).expect("fits");
                let header = u32::try_from(size_of::<RAWINPUTHEADER>()).expect("fits");

                let read = GetRawInputData(
                    HRAWINPUT(message.lParam.0 as *mut _),
                    RID_INPUT,
                    Some(buffer.as_mut_ptr().cast()),
                    &raw mut size,
                    header,
                );
                if read == 0 || read == u32::MAX {
                    continue;
                }

                let raw = buffer.as_ptr().cast::<RAWINPUT>().read_unaligned();
                match raw.header.dwType {
                    KEYBOARD => events.push(KeyEvent::Keyboard {
                        make_code: raw.data.keyboard.MakeCode,
                        flags: raw.data.keyboard.Flags,
                        vkey: raw.data.keyboard.VKey,
                        message: raw.data.keyboard.Message,
                    }),
                    HID => {
                        let count = raw.data.hid.dwCount as usize;
                        let each = raw.data.hid.dwSizeHid as usize;
                        let at = std::mem::offset_of!(RAWINPUT, data)
                            + std::mem::offset_of!(windows::Win32::UI::Input::RAWHID, bRawData);
                        let end = (at + count * each).min(buffer.len());
                        events.push(KeyEvent::Hid {
                            device: self.device_name(raw.header.hDevice.0 as isize),
                            bytes: buffer[at..end].to_vec(),
                        });
                    }
                    _ => {}
                }

                DefWindowProcW(self.window, message.message, message.wParam, message.lParam);
            }
        }

        events
    }

    /// The device path, asked for once per device and remembered.
    fn device_name(&mut self, handle: isize) -> String {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::UI::Input::{GetRawInputDeviceInfoW, RIDI_DEVICENAME};

        if let Some(known) = self.names.get(&handle) {
            return known.clone();
        }

        let mut name = String::from("<unknown device>");
        unsafe {
            let mut length = 0_u32;
            let device = HANDLE(handle as *mut _);
            GetRawInputDeviceInfoW(Some(device), RIDI_DEVICENAME, None, &raw mut length);
            if length > 0 && length < 1024 {
                let mut text = vec![0_u16; length as usize];
                let read = GetRawInputDeviceInfoW(
                    Some(device),
                    RIDI_DEVICENAME,
                    Some(text.as_mut_ptr().cast()),
                    &raw mut length,
                );
                if read != u32::MAX {
                    name = String::from_utf16_lossy(&text)
                        .trim_end_matches('\0')
                        .to_string();
                }
            }
        }

        self.names.insert(handle, name.clone());
        name
    }
}
