#[path = "../../../src-tauri/src/window_icon.rs"]
mod window_icon;

use std::{mem, ptr};
use winapi::{
    shared::windef::HICON,
    um::{libloaderapi::GetModuleHandleW, wingdi::DeleteObject, winuser::*},
};

fn valid(icon: HICON) -> bool {
    unsafe {
        let mut info = mem::zeroed();
        if GetIconInfo(icon, &mut info) == 0 {
            return false;
        }
        DeleteObject(info.hbmMask as _);
        DeleteObject(info.hbmColor as _);
        true
    }
}

fn main() {
    unsafe {
        let foreground = GetForegroundWindow();
        let class: Vec<u16> = "Pullcept icon isolated check\0".encode_utf16().collect();
        let module = GetModuleHandleW(ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            hInstance: module,
            lpszClassName: class.as_ptr(),
            ..mem::zeroed()
        };
        assert_ne!(RegisterClassW(&wc), 0);
        let hwnd = CreateWindowExW(
            WS_EX_NOACTIVATE,
            class.as_ptr(),
            class.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            -10000,
            -10000,
            100,
            100,
            ptr::null_mut(),
            ptr::null_mut(),
            module,
            ptr::null_mut(),
        );
        assert!(!hwnd.is_null());
        // Match Tao's separate small/large channels and its empty class icons.
        let small = LoadImageW(module, MAKEINTRESOURCEW(32512), IMAGE_ICON, 16, 16, 0);
        assert!(!small.is_null());
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as _, small as _);
        assert_eq!(SendMessageW(hwnd, WM_GETICON, ICON_BIG as _, 0), 0);
        assert_eq!(GetClassLongPtrW(hwnd, GCLP_HICON), 0);
        window_icon::set_taskbar_icon(hwnd).unwrap();
        let large = SendMessageW(hwnd, WM_GETICON, ICON_BIG as _, 0) as HICON;
        let expected = LoadImageW(
            module,
            MAKEINTRESOURCEW(32512),
            IMAGE_ICON,
            0,
            0,
            LR_SHARED | LR_DEFAULTSIZE,
        );
        assert_eq!(large as winapi::shared::ntdef::HANDLE, expected);
        assert!(valid(large));
        assert_eq!(
            SendMessageW(hwnd, WM_GETICON, ICON_SMALL as _, 0),
            small as _
        );
        for _ in 0..100 {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            ShowWindow(hwnd, SW_HIDE);
            SetWindowTextW(hwnd, class.as_ptr());
            let mut msg = mem::zeroed();
            while PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            assert_eq!(SendMessageW(hwnd, WM_GETICON, ICON_BIG as _, 0), large as _);
            assert!(valid(large));
        }
        DestroyWindow(hwnd);
        DestroyIcon(small as _);
        assert!(valid(large));
        assert_eq!(GetForegroundWindow(), foreground);
        println!("PASS: embedded resource 32512, actual production helper, unchanged small icon, large icon valid after return + 100 show/minimize/restore/hide/title cycles + HWND destruction; foreground unchanged");
    }
}
