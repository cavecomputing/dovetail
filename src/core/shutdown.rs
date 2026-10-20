//! The save that still happens when the app is ended without its window closing first: logging
//! out, shutting down, `kill`, Ctrl+C in the terminal, the display server going away, and on
//! macOS Quit from the Dock. The window keeps the text autosave hasn't written yet here, and
//! `write_unsaved` writes it from whichever thread hears the app is ending.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::core::opened::{Doc, write_doc};
use crate::core::scratch::ScratchView;
use crate::core::text_file::DiskStamp;

/// Text the window has that its document on disk doesn't, with what writing it needs.
pub struct Unsaved {
    pub doc: Doc,
    pub text: String,
    pub view: ScratchView,
    pub data_dir: PathBuf,
    pub disk_stamp: Option<DiskStamp>,
}

static UNSAVED: Mutex<Option<Unsaved>> = Mutex::new(None);

/// Keeps `unsaved` until the next `remember`, `forget` or `write_unsaved`.
pub fn remember(unsaved: Unsaved) {
    *UNSAVED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(unsaved);
}

/// Everything is on disk: nothing to write at the end.
pub fn forget() {
    UNSAVED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
}

/// Writes the text autosave hasn't yet, if there is any. Only the first call writes.
pub fn write_unsaved() {
    let unsaved = UNSAVED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(unsaved) = unsaved {
        let written = write_doc(
            &unsaved.doc,
            unsaved.text,
            unsaved.view,
            &unsaved.data_dir,
            unsaved.disk_stamp,
        );
        if let Err(reason) = written {
            eprintln!("Couldn't save before quitting: {reason}");
        }
    }
}

/// Listens for the OS ending the app, and writes the unsaved text before it goes. Call once, at
/// startup.
pub fn write_unsaved_at_end() {
    // Linux and macOS: the session sends SIGTERM at logout and shutdown, a closed terminal sends
    // SIGHUP and Ctrl+C sends SIGINT. Each would end the app at once, so a thread waits for them
    // instead, saves, and exits. macOS's Dock Quit and logout end the app through exit(), which
    // runs the atexit handler.
    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

        extern "C" fn at_exit() {
            write_unsaved();
        }
        // SAFETY: at_exit is a plain function that takes nothing and returns nothing, as atexit
        // asks.
        unsafe { libc::atexit(at_exit) };

        if let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGTERM, SIGHUP, SIGINT]) {
            std::thread::spawn(move || {
                if let Some(signal) = signals.forever().next() {
                    write_unsaved();
                    std::process::exit(128 + signal);
                }
            });
        }
    }
    // Windows: logging out and shutting down send WM_ENDSESSION to every top-level window, then
    // end the process as soon as it is answered. winit doesn't pass it on, so a hidden window of
    // our own, on its own thread, answers it after saving.
    #[cfg(windows)]
    std::thread::spawn(session_end_window);
}

#[cfg(windows)]
fn session_end_window() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW,
        WM_ENDSESSION, WNDCLASSW,
    };

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // A wparam of 0 means the session isn't ending after all.
        if message == WM_ENDSESSION && wparam != 0 {
            write_unsaved();
        }
        // SAFETY: passes on the arguments Windows gave this procedure.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    let class_name: Vec<u16> = "SessionEnd\0".encode_utf16().collect();
    // SAFETY: the class and window are made once, on this thread, from a name that lives until the
    // thread ends; the window is never shown, and this thread only pumps its messages.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };
        if RegisterClassW(&class) == 0 {
            return;
        }
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if window.is_null() {
            return;
        }
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&message);
        }
    }
}
