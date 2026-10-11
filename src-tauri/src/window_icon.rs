use std::{io, ptr};
use winapi::{
    shared::windef::HWND,
    um::{
        libloaderapi::GetModuleHandleW,
        winuser::{
            LoadImageW, SendMessageW, ICON_BIG, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED,
            MAKEINTRESOURCEW, WM_SETICON,
        },
    },
};

/// Supply the large icon that Tauri's small-window-icon path leaves unset.
pub fn set_taskbar_icon(hwnd: HWND) -> io::Result<()> {
    // tauri-build embeds bundle.icon's ICO as resource 32512 in both dev and
    // release executables. A shared standard-size resource is owned by Windows,
    // so the HWND does not keep a handle that a temporary Rust owner destroys.
    unsafe {
        let module = GetModuleHandleW(ptr::null());
        if module.is_null() {
            return Err(io::Error::last_os_error());
        }
        let icon = LoadImageW(
            module,
            MAKEINTRESOURCEW(32512),
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE | LR_SHARED,
        );
        if icon.is_null() {
            return Err(io::Error::last_os_error());
        }
        // The return value is the previous icon, not a success flag. Leave the
        // small icon and its lifetime under Tao's WindowState ownership.
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon as isize);
    }
    Ok(())
}
