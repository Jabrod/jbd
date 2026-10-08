//! Windows: give a window the app icon at the sizes the taskbar and Alt-Tab use.
//!
//! eframe (through winit) sets only the window's *small* icon from a PNG. The taskbar asks for the
//! *big* icon, finds none, falls back to the window class (which winit registers without one) and
//! shows Windows' generic program icon. This loads icon resource 1 (the `.ico` that
//! `apps/filmcraft/build.rs` embeds in jbd.exe, with its hand-tuned taskbar frames) at the system's
//! big and small icon sizes and hands both to the window with `WM_SETICON`.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
};

/// The resource id `winresource::WindowsResource::set_icon` gives the app icon.
const APP_ICON_ID: usize = 1;

/// Set the big and small icons of window `hwnd` from this program's icon resource. Returns false
/// when the program has no icon resource (e.g. built without a resource compiler) or `hwnd` is 0.
pub fn set_window_icons_from_exe(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    // SAFETY: GetModuleHandleW(null) returns this process's own module handle and has no
    // preconditions; it does not need to be released.
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    if module.is_null() {
        return false;
    }
    let load = |cx, cy| {
        // SAFETY: `module` is a valid module handle; MAKEINTRESOURCEW(1) is the integer resource id
        // passed as a pointer value (never dereferenced as a string by LoadImageW); the sizes come
        // from GetSystemMetrics. A missing resource returns null, which is checked below.
        unsafe { LoadImageW(module, APP_ICON_ID as *const u16, IMAGE_ICON, GetSystemMetrics(cx), GetSystemMetrics(cy), LR_DEFAULTCOLOR) }
    };
    let big = load(SM_CXICON, SM_CYICON);
    let small = load(SM_CXSMICON, SM_CYSMICON);
    let window = hwnd as HWND;
    let mut any = false;
    for (kind, icon) in [(ICON_BIG, big), (ICON_SMALL, small)] {
        if !icon.is_null() {
            // SAFETY: `window` is the caller's window handle (an invalid one makes SendMessageW
            // fail harmlessly); WM_SETICON takes the icon handle as LPARAM. Icons loaded with
            // LoadImageW without LR_SHARED stay valid for the window's lifetime; they are freed
            // when the process exits.
            unsafe { SendMessageW(window, WM_SETICON, kind as usize, icon as isize) };
            any = true;
        }
    }
    any
}
