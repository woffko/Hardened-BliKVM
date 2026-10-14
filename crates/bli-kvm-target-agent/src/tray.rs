//! The agent's icon in the notification area: its name, version and commit
//! and whether it is connected to the board, on the icon and in its menu,
//! what it reads shown on request, and Quit. One agent runs in a user's
//! session; a second start leaves the first. The icon comes back when the
//! taskbar is started again. Windows' calls are this module's unsafe code.

use std::mem::zeroed;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};

use bli_kvm_target_agent::names::describe;
use bli_kvm_target_agent::{title, wide};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, IDI_APPLICATION, LoadIconW, MB_ICONINFORMATION,
    MB_OK, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, TPM_RIGHTBUTTON, TrackPopupMenu,
    TranslateMessage, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WM_NULL,
    WM_RBUTTONUP, WNDCLASSW, WS_OVERLAPPED,
};

/// What the icon sends its window when it is clicked.
const ICON_MESSAGE: u32 = WM_APP + 1;
/// The menu's Quit.
const QUIT: usize = 1;
/// The menu's "Show what the agent reads".
const SHOW: usize = 2;
/// What the channel sends the window when its status changes.
const STATUS_MESSAGE: u32 = WM_APP + 2;
/// The icon's number among the window's icons.
const ICON_ID: u32 = 1;
/// The message the taskbar sends every window when it starts again.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// Shows the icon and runs until Quit; the process's exit code.
pub fn run() -> i32 {
    // SAFETY: each call gets what Windows documents for it: zero-ended UTF-16
    // that outlives the call, structures zeroed and then filled, and the
    // window procedure below for the class it registers.
    unsafe {
        let name = wide("Local\\HardenedBliKVMAgent", 64);
        let mutex = CreateMutexW(null(), 0, name.as_ptr());
        if mutex.is_null() {
            return 1;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Another agent runs in this session.
            return 0;
        }
        let instance = GetModuleHandleW(null());
        let class = wide("HardenedBliKVMAgent", 64);
        let mut window_class: WNDCLASSW = zeroed();
        window_class.lpfnWndProc = Some(window_procedure);
        window_class.hInstance = instance;
        window_class.lpszClassName = class.as_ptr();
        if RegisterClassW(&window_class) == 0 {
            return 1;
        }
        let caption = wide(&title(), 128);
        let window = CreateWindowExW(
            0,
            class.as_ptr(),
            caption.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if window.is_null() {
            return 1;
        }
        let restarted = wide("TaskbarCreated", 32);
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(restarted.as_ptr()),
            Ordering::Relaxed,
        );
        if !add_icon(window) {
            DestroyWindow(window);
            return 1;
        }
        crate::channel::start(window as isize, STATUS_MESSAGE);
        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        0
    }
}

/// The icon's description for `Shell_NotifyIconW`.
unsafe fn icon_data(window: HWND) -> NOTIFYICONDATAW {
    // SAFETY: a zeroed NOTIFYICONDATAW is valid; the fields set are its own.
    unsafe {
        let mut data: NOTIFYICONDATAW = zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = window;
        data.uID = ICON_ID;
        data
    }
}

/// The icon's tip: the agent's title and its status.
fn tip() -> String {
    format!("{}\n{}", title(), crate::channel::status())
}

unsafe fn add_icon(window: HWND) -> bool {
    // SAFETY: as in `run`; the tip is copied into the structure.
    unsafe {
        let mut data = icon_data(window);
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = ICON_MESSAGE;
        data.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
        let tip = wide(&tip(), data.szTip.len());
        data.szTip[..tip.len()].copy_from_slice(&tip);
        Shell_NotifyIconW(NIM_ADD, &data) != 0
    }
}

unsafe fn update_tip(window: HWND) {
    // SAFETY: as in `add_icon`.
    unsafe {
        let mut data = icon_data(window);
        data.uFlags = NIF_TIP;
        let tip = wide(&tip(), data.szTip.len());
        data.szTip[..tip.len()].copy_from_slice(&tip);
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

/// What the agent reads, in a message box, as the board would be told it.
unsafe fn show_reads(window: HWND) {
    let text = describe(
        &crate::reads::system(),
        &crate::reads::monitors(),
        &crate::reads::keyboard(),
    );
    let text = wide(&text, 8192);
    let caption = wide(&title(), 128);
    // SAFETY: zero-ended texts that outlive the call.
    unsafe {
        MessageBoxW(
            window,
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

unsafe fn remove_icon(window: HWND) {
    // SAFETY: as in `run`.
    unsafe {
        let data = icon_data(window);
        Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// The icon's menu at the pointer: the agent's title and status, greyed,
/// what it reads, and Quit.
unsafe fn show_menu(window: HWND) {
    // SAFETY: as in `run`; the menu is destroyed after it was shown.
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        let caption = wide(&title(), 128);
        let status = wide(&crate::channel::status(), 160);
        let show = wide("Show what the agent reads", 64);
        let quit = wide("Quit", 16);
        AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, caption.as_ptr());
        AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, status.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, SHOW, show.as_ptr());
        AppendMenuW(menu, MF_STRING, QUIT, quit.as_ptr());
        let mut point: POINT = zeroed();
        GetCursorPos(&mut point);
        // Without it the menu would not close when clicked beside.
        SetForegroundWindow(window);
        TrackPopupMenu(menu, TPM_RIGHTBUTTON, point.x, point.y, 0, window, null());
        PostMessageW(window, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: called by Windows for this class's window, with its arguments.
    unsafe {
        match message {
            ICON_MESSAGE => {
                let event = (lparam as u32) & 0xffff;
                if event == WM_RBUTTONUP || event == WM_LBUTTONUP || event == WM_CONTEXTMENU {
                    show_menu(window);
                }
                0
            }
            WM_COMMAND => {
                match wparam & 0xffff {
                    QUIT => {
                        DestroyWindow(window);
                    }
                    SHOW => show_reads(window),
                    _ => {}
                }
                0
            }
            STATUS_MESSAGE => {
                update_tip(window);
                0
            }
            WM_DESTROY => {
                remove_icon(window);
                PostQuitMessage(0);
                0
            }
            _ if message != 0 && message == TASKBAR_CREATED.load(Ordering::Relaxed) => {
                add_icon(window);
                0
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}
