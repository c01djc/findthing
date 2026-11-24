use std::ffi::c_void;
use std::mem::{size_of, zeroed};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{w, Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, COLORREF, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT,
    WPARAM, ERROR_ALREADY_EXISTS,
};
use windows::Win32::Security::{
    InitializeSecurityDescriptor, SetSecurityDescriptorDacl, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    SECURITY_DESCRIPTOR,
};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, ClientToScreen, CreateBitmap, CreateDIBSection, CreateFontW, CreateRoundRectRgn,
    CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, GetMonitorInfoW, InvalidateRect,
    MonitorFromPoint,
    SelectObject, SetBkMode, SetTextColor, SetWindowRgn, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_LEFT,
    DT_SINGLELINE, DT_VCENTER, FW_NORMAL, MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::TextServices::{
    ITfCompartmentMgr, ITfThreadMgr, CLSID_TF_ThreadMgr, GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION,
    GUID_COMPARTMENT_KEYBOARD_OPENCLOSE,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Threading::{
    AttachThreadInput, CreateMutexW, GetCurrentProcessId, GetCurrentThreadId, OpenProcess,
    QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::Input::Ime::{
    ImmGetContext, ImmGetConversionStatus, ImmReleaseContext, ImmSetConversionStatus,
    ImmSetOpenStatus, IME_CMODE_NATIVE, IME_CONVERSION_MODE, IME_SENTENCE_MODE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ActivateKeyboardLayout, GetAsyncKeyState, GetKeyboardLayout, GetKeyboardLayoutList, MapVirtualKeyW,
    SendInput, SetFocus, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KLF_SETFORPROCESS, MAPVK_VSC_TO_VK_EX, VIRTUAL_KEY, VK_BACK, VK_CONTROL,
    VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RWIN, VK_SHIFT,
    VK_SPACE,
};
use windows::Win32::UI::Shell::{
    ITaskbarList, SHBrowseForFolderW, SHFileOperationW, SHGetPathFromIDListW, ShellExecuteW,
    Shell_NotifyIconW, TaskbarList,
    BIF_EDITBOX, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, FO_DELETE, FO_MOVE,
    FOF_ALLOWUNDO, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    SHFILEOPSTRUCTW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreateIconIndirect, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyMenu, DispatchMessageW,     GetAncestor, GetClassNameW, GetClientRect, GetCursorPos, GetForegroundWindow,
    GetGUIThreadInfo, GetMessageW, GetParent, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    AllowSetForegroundWindow, LoadCursorW, LoadImageW, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SetCursorPos, SetForegroundWindow, SetWindowLongPtrW,
    SendMessageW, SetTimer, SetWindowPos, SetWindowsHookExW, ShowWindow, TrackPopupMenu,
    TranslateMessage, UnhookWindowsHookEx, ChangeWindowMessageFilter, CS_HREDRAW, CS_VREDRAW, GA_ROOT, GUITHREADINFO, HHOOK,
    HWND_BROADCAST, ICONINFO, IDC_ARROW, IMAGE_ICON, KBDLLHOOKSTRUCT,
    KBDLLHOOKSTRUCT_FLAGS, LR_SHARED, MSLLHOOKSTRUCT, MSGFLT_ADD,
    GWL_EXSTYLE, GWL_STYLE, GW_OWNER, MF_CHECKED, MF_POPUP, MF_STRING, MSG, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNORMAL,
    SW_SHOWNOACTIVATE,
    TPM_BOTTOMALIGN, TPM_RIGHTALIGN, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_EX_STYLE, WM_APP, WM_COMMAND,
    WM_INPUTLANGCHANGEREQUEST, WM_LBUTTONUP, WM_NULL, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SETICON, WM_TIMER,
    WNDCLASSEXW,
    WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_SYSMENU,
    HWND_TOPMOST,
};

use crate::locate;

const WM_TRAY: u32 = WM_APP + 1;
const WM_QUERY: u32 = WM_APP + 2;
const WM_FOUND: u32 = WM_APP + 3;
const WM_TOGGLE: u32 = WM_APP + 4;
const LLKHF_UP: u32 = 0x80;
const LLKHF_ALTDOWN: u32 = 0x20;

#[derive(Clone, Copy)]
pub enum ShellEvent {
    Show,
    Hide,
    Quit,
    Capture,
    CaptureDone,
    Theme,
    Folder(u64),
    Save,
}

const MOD_CTRL: u32 = 1;
const MOD_ALT: u32 = 2;
const MOD_SHIFT: u32 = 4;
const MOD_WIN: u32 = 8;
const HOTKEY_ID: i32 = 1;
const WIN_MOD_ALT: u32 = 0x0001;
const WIN_MOD_CONTROL: u32 = 0x0002;
const WIN_MOD_SHIFT: u32 = 0x0004;
const WIN_MOD_WIN: u32 = 0x0008;
const WIN_MOD_NOREPEAT: u32 = 0x4000;
const WM_HOTKEY: u32 = 0x0312;

#[link(name = "user32")]
extern "system" {
    fn RegisterHotKey(hwnd: *mut c_void, id: i32, mods: u32, vk: u32) -> i32;
    fn UnregisterHotKey(hwnd: *mut c_void, id: i32) -> i32;
    fn SetWinEventHook(
        event_min: u32,
        event_max: u32,
        module: *mut c_void,
        proc: unsafe extern "system" fn(*mut c_void, u32, HWND, i32, i32, u32, u32),
        pid: u32,
        tid: u32,
        flags: u32,
    ) -> *mut c_void;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hotkey {
    DoubleCtrl,
    Chord(u32, u32),
}

static INSTANCE_MUTEX: AtomicIsize = AtomicIsize::new(0);
static LAUNCHER_VISIBLE: AtomicBool = AtomicBool::new(false);
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
static PILL_HWND: AtomicIsize = AtomicIsize::new(0);
static HOOK: AtomicIsize = AtomicIsize::new(0);
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);
static LAUNCHER_HWND: AtomicIsize = AtomicIsize::new(0);
static SHOW_MSG: AtomicU32 = AtomicU32::new(0);
static GENERATION: AtomicU64 = AtomicU64::new(0);
static HELD: AtomicU32 = AtomicU32::new(0);
static MOD_HELD: AtomicU32 = AtomicU32::new(0);
static HOTKEY_REGISTERED: AtomicBool = AtomicBool::new(false);
static ACCEPT_INJECTED: AtomicBool = AtomicBool::new(false);
static LAST_CTRL_UP: AtomicU32 = AtomicU32::new(0);
static CTRL_AS_MOD: AtomicBool = AtomicBool::new(false);
static CAPTURE: AtomicBool = AtomicBool::new(false);
static LAUNCHER_FOCUSED: AtomicBool = AtomicBool::new(false);
static HOTKEY: Mutex<Hotkey> = Mutex::new(Hotkey::DoubleCtrl);
static THEME: Mutex<Theme> = Mutex::new(Theme::Paper);
static QUERY: Mutex<String> = Mutex::new(String::new());
static PILL: Mutex<Pill> = Mutex::new(Pill { text: String::new(), found: false });
static DISPATCH: OnceLock<Box<dyn Fn(ShellEvent) + Send + Sync>> = OnceLock::new();
static LOCATE_TX: OnceLock<Sender<String>> = OnceLock::new();
static FOLDER_KEYS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
static FOLDER_GEN: AtomicU64 = AtomicU64::new(1);
static FOLDER_SYNC: AtomicBool = AtomicBool::new(false);
static EXPLORER_LIST: AtomicBool = AtomicBool::new(false);
static FOCUS_EDITOR: AtomicBool = AtomicBool::new(false);
static KNOWN_EXPLORER: AtomicIsize = AtomicIsize::new(0);
static KEY_SEQ: AtomicU32 = AtomicU32::new(0);
static KEY_VK: AtomicU32 = AtomicU32::new(0);
static KEY_ACT: AtomicU32 = AtomicU32::new(0);
static REPLAY_LEFT: AtomicU32 = AtomicU32::new(0);
const REPLAY_MARK: usize = 0x464E_4454;
static EXPLICIT_SEARCH: AtomicBool = AtomicBool::new(false);
static FOLDER_ARMING: AtomicBool = AtomicBool::new(false);
static FOLDER_SAW_TEXT: AtomicBool = AtomicBool::new(false);
static FOLDER_BACKS: AtomicU32 = AtomicU32::new(0);
static ANCHOR_FREE: AtomicBool = AtomicBool::new(false);

struct Pill {
    text: String,
    found: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Theme {
    Paper,
    Field,
    Ink,
    Split,
    Ledger,
}

pub fn enable_dpi() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn set_launcher_visible(open: bool) {
    LAUNCHER_VISIBLE.store(open, Ordering::Relaxed);
}

pub fn cancel_capture() {
    CAPTURE.store(false, Ordering::Relaxed);
}

pub fn load_theme() {
    let Ok(text) = std::fs::read_to_string(theme_path()) else {
        return;
    };
    *theme_slot() = parse_theme(text.trim());
}

pub fn theme_name() -> String {
    theme_id(*theme_slot()).to_string()
}

pub fn summon_parts() -> (bool, Vec<String>) {
    match current_hotkey() {
        Hotkey::DoubleCtrl => (true, vec!["Ctrl".to_string()]),
        Hotkey::Chord(mods, vk) => (false, chord_parts(mods, vk)),
    }
}

pub fn allow_summon_from_lower_integrity() {
    let message = unsafe { RegisterWindowMessageW(w!("Findthing.Show")) };
    if message != 0 {
        let _ = unsafe { ChangeWindowMessageFilter(message, MSGFLT_ADD) };
    }
}

pub fn notify_running_instance() -> bool {
    let mut descriptor = SECURITY_DESCRIPTOR::default();
    let mutex = unsafe {
        let descriptor = PSECURITY_DESCRIPTOR(&mut descriptor as *mut _ as *mut c_void);
        let ready = InitializeSecurityDescriptor(descriptor, 1).is_ok()
            && SetSecurityDescriptorDacl(descriptor, true, None, false).is_ok();
        if ready {
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            CreateMutexW(Some(&attributes), true, w!("Local\\Findthing.Single"))
        } else {
            CreateMutexW(None, true, w!("Local\\Findthing.Single"))
        }
    };
    let Ok(_mutex) = mutex else {
        return false;
    };
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        INSTANCE_MUTEX.store(_mutex.0 as isize, Ordering::Relaxed);
        return false;
    }
    let message = unsafe { RegisterWindowMessageW(w!("Findthing.Show")) };
    for _ in 0..8 {
        unsafe {
            let _ = PostMessageW(Some(HWND_BROADCAST), message, WPARAM(0), LPARAM(0));
        }
        thread::sleep(Duration::from_millis(40));
    }
    true
}

pub fn spawn_shell(dispatch: impl Fn(ShellEvent) + Send + Sync + 'static) {
    let _ = DISPATCH.set(Box::new(dispatch));
    thread::Builder::new()
        .name("findthing-shell".into())
        .spawn(shell_thread)
        .ok();
}

pub fn raise_launcher(window: &slint::Window) {
    if !LAUNCHER_VISIBLE.load(Ordering::Relaxed) {
        return;
    }
    let Some(hwnd) = native_hwnd(window) else {
        return;
    };
    LAUNCHER_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let desired = (ex | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0) & !WS_EX_APPWINDOW.0;
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let style_desired = style & !WS_SYSMENU.0;
        if ex == desired && style == style_desired {
            return;
        }
        if style != style_desired {
            SetWindowLongPtrW(hwnd, GWL_STYLE, style_desired as isize);
        }
        if ex != desired {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, desired as isize);
        }
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        if let Ok(taskbar) = CoCreateInstance::<_, ITaskbarList>(&TaskbarList, None, CLSCTX_ALL) {
            if taskbar.HrInit().is_ok() {
                let _ = taskbar.DeleteTab(hwnd);
            }
        }
    }
}

pub fn move_window(window: &slint::Window, x: i32, y: i32, width: i32, height: i32) {
    let Some(hwnd) = native_hwnd(window) else {
        return;
    };
    let width = width.max(1);
    let height = height.max(1);
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_ok()
            && rect.left == x
            && rect.top == y
            && rect.right - rect.left == width
            && rect.bottom - rect.top == height
        {
            return;
        }
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

pub fn prepare_launcher(window: &slint::Window) {
    LAUNCHER_FOCUSED.store(false, Ordering::Relaxed);
    raise_launcher(window);
    let Some(hwnd) = native_hwnd(window) else {
        return;
    };
    unsafe {
        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let current = GetCurrentThreadId();
        if foreground_thread != 0 && foreground_thread != current {
            let _ = AttachThreadInput(foreground_thread, current, true);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            let _ = AttachThreadInput(foreground_thread, current, false);
        } else {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
        }
        prefer_chinese_input(window);
        if folder_synced() {
            return;
        }
        // A click that arrives before the next mouse-move uses a stale position.
        let mut point = POINT::default();
        if GetCursorPos(&mut point).is_ok() {
            let _ = SetCursorPos(point.x, point.y);
        }
    }
}

pub fn hold_launcher(ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    let mut slot = hold_until().lock().unwrap_or_else(|err| err.into_inner());
    *slot = Some(until);
}

pub fn extend_hold(ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    let mut slot = hold_until().lock().unwrap_or_else(|err| err.into_inner());
    if slot.map(|current| current < until).unwrap_or(true) {
        *slot = Some(until);
    }
}

fn launcher_held() -> bool {
    hold_until()
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .map(|until| Instant::now() < until)
        .unwrap_or(false)
}

fn hold_until() -> &'static Mutex<Option<Instant>> {
    static HOLD: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    HOLD.get_or_init(|| Mutex::new(None))
}

pub fn focus_launcher(window: &slint::Window) {
    raise_launcher(window);
    let Some(hwnd) = native_hwnd(window) else {
        return;
    };
    unsafe {
        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let current = GetCurrentThreadId();
        if foreground_thread != 0 && foreground_thread != current {
            let _ = AttachThreadInput(foreground_thread, current, true);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            let _ = AttachThreadInput(foreground_thread, current, false);
        } else {
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
        }
    }
}

pub fn cursor_over_window(window: &slint::Window) -> bool {
    let Some(hwnd) = native_hwnd(window) else {
        return false;
    };
    let mut now = POINT::default();
    if unsafe { GetCursorPos(&mut now) }.is_err() {
        return false;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return false;
    }
    now.x >= rect.left && now.x < rect.right && now.y >= rect.top && now.y < rect.bottom
}

pub fn take_folder_keys() -> Vec<u32> {
    let mut keys = FOLDER_KEYS.lock().unwrap_or_else(|err| err.into_inner());
    let taken = std::mem::take(&mut *keys);
    if taken.is_empty() {
        FOLDER_ARMING.store(false, Ordering::Relaxed);
    }
    taken
}

pub fn replay_folder_input(backs: u32, keys: &[u32]) {
    if backs == 0 && keys.is_empty() {
        return;
    }
    let mut inputs = Vec::with_capacity(((backs as usize) + keys.len()) * 2);
    for _ in 0..backs {
        inputs.push(key_stroke(VK_BACK.0 as u16, false));
        inputs.push(key_stroke(VK_BACK.0 as u16, true));
    }
    for &vk in keys {
        inputs.push(key_stroke(vk as u16, false));
        inputs.push(key_stroke(vk as u16, true));
    }
    unsafe {
        REPLAY_LEFT.store(inputs.len() as u32, Ordering::SeqCst);
        let _ = SendInput(&inputs, size_of::<INPUT>() as i32);
        REPLAY_LEFT.store(0, Ordering::SeqCst);
    }
}

fn key_stroke(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: REPLAY_MARK,
            },
        },
    }
}

pub fn prefer_chinese_input(window: &slint::Window) {
    let Some(hwnd) = native_hwnd(window) else {
        return;
    };
    unsafe {
        let current = GetKeyboardLayout(0);
        if !chinese_lang(current) {
            let mut layouts = [HKL::default(); 32];
            let count = GetKeyboardLayoutList(Some(&mut layouts));
            if count > 0 && (count as usize) <= layouts.len() {
                let listed = &layouts[..count as usize];
                let chinese = listed
                    .iter()
                    .copied()
                    .find(|layout| chinese_ime(*layout))
                    .or_else(|| listed.iter().copied().find(|layout| chinese_lang(*layout)));
                if let Some(layout) = chinese {
                    let _ = PostMessageW(
                        Some(hwnd),
                        WM_INPUTLANGCHANGEREQUEST,
                        WPARAM(0),
                        LPARAM(layout.0 as isize),
                    );
                    let _ = ActivateKeyboardLayout(layout, KLF_SETFORPROCESS);
                }
            }
        }
        let context = ImmGetContext(hwnd);
        if !context.0.is_null() {
            let _ = ImmSetOpenStatus(context, true);
            let mut conversion = IME_CONVERSION_MODE::default();
            let mut sentence = IME_SENTENCE_MODE::default();
            if ImmGetConversionStatus(context, Some(&mut conversion), Some(&mut sentence)).as_bool()
            {
                conversion |= IME_CMODE_NATIVE;
                let _ = ImmSetConversionStatus(context, conversion, sentence);
            }
            let _ = ImmReleaseContext(hwnd, context);
        }
        set_pinyin_native();
    }
}

fn set_pinyin_native() {
    unsafe {
        let Ok(manager) = CoCreateInstance::<_, ITfThreadMgr>(&CLSID_TF_ThreadMgr, None, CLSCTX_ALL) else {
            return;
        };
        let Ok(client) = manager.Activate() else {
            return;
        };
        let Ok(compartments) = manager.cast::<ITfCompartmentMgr>() else {
            let _ = manager.Deactivate();
            return;
        };
        let native = ime_mode(1);
        if let Ok(open) = compartments.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE) {
            let _ = open.SetValue(client, &native);
        }
        if let Ok(mode) = compartments.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION) {
            let _ = mode.SetValue(client, &native);
        }
        let _ = manager.Deactivate();
    }
}

fn ime_mode(value: i32) -> VARIANT {
    unsafe {
        let mut variant = VARIANT::default();
        let header = &mut *variant.Anonymous.Anonymous;
        header.vt = VT_I4;
        header.Anonymous.lVal = value;
        variant
    }
}

fn chinese_lang(layout: HKL) -> bool {
    (layout.0 as usize as u32 & 0x3ff) == 0x04
}

fn chinese_ime(layout: HKL) -> bool {
    let value = layout.0 as usize as u32;
    chinese_lang(layout) && (value >> 16) != 0 && (value & 0xffff) == 0x0804
}

pub fn clicked_outside(window: &slint::Window) -> bool {
    if launcher_held() {
        return false;
    }
    let Some(ours) = native_hwnd(window) else {
        return false;
    };
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null()
        || foreground == ours
        || shares_input(ours, foreground)
        || taskbar_or_menu(foreground)
        || pinned_explorer(foreground)
    {
        if foreground == ours {
            LAUNCHER_FOCUSED.store(true, Ordering::Relaxed);
        }
        return false;
    }
    if FOLDER_SYNC.load(Ordering::Relaxed) {
        return true;
    }
    LAUNCHER_FOCUSED.load(Ordering::Relaxed)
}

pub fn pinned_folder_left(started: &str) -> bool {
    if !FOLDER_SYNC.load(Ordering::Relaxed) || started.is_empty() {
        return false;
    }
    let Some(path) = locate::foreground_folder() else {
        return false;
    };
    let current = path.to_string_lossy();
    !same_folder(started, current.as_ref())
}

fn same_folder(started: &str, current: &str) -> bool {
    let started = started.trim_end_matches(['\\', '/']);
    let current = current.trim_end_matches(['\\', '/']);
    started.eq_ignore_ascii_case(current)
}

pub fn pinned_window_gone() -> bool {
    if !FOLDER_SYNC.load(Ordering::Relaxed) {
        return false;
    }
    let pin = locate::pinned_window();
    if pin == 0 {
        return false;
    }
    let hwnd = hwnd_from(pin);
    unsafe {
        !IsWindow(Some(hwnd)).as_bool()
            || IsIconic(hwnd).as_bool()
            || !IsWindowVisible(hwnd).as_bool()
    }
}

fn pinned_explorer(hwnd: HWND) -> bool {
    if !FOLDER_SYNC.load(Ordering::Relaxed) {
        return false;
    }
    let pin = locate::pinned_window();
    if pin == 0 || hwnd.0.is_null() {
        return false;
    }
    let mut current = hwnd;
    for _ in 0..12 {
        if current.0 as isize == pin {
            return true;
        }
        match unsafe { GetParent(current) } {
            Ok(parent) if !parent.0.is_null() && parent != current => current = parent,
            _ => break,
        }
    }
    false
}

struct WindowDrag {
    active: bool,
    cursor: POINT,
    origin: POINT,
}

fn drag_state() -> &'static Mutex<WindowDrag> {
    static DRAG: OnceLock<Mutex<WindowDrag>> = OnceLock::new();
    DRAG.get_or_init(|| {
        Mutex::new(WindowDrag {
            active: false,
            cursor: POINT::default(),
            origin: POINT::default(),
        })
    })
}

pub fn begin_drag(window: &slint::Window) {
    let Some(hwnd) = native_hwnd(window) else {
        return;
    };
    unsafe {
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor).is_err() {
            return;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return;
        }
        let mut drag = drag_state().lock().unwrap_or_else(|err| err.into_inner());
        drag.active = true;
        drag.cursor = cursor;
        drag.origin = POINT { x: rect.left, y: rect.top };
    }
}

pub fn drag_move(window: &slint::Window) {
    let Some(hwnd) = native_hwnd(window) else {
        return;
    };
    let drag = drag_state().lock().unwrap_or_else(|err| err.into_inner());
    if !drag.active {
        return;
    }
    unsafe {
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor).is_err() {
            return;
        }
        let x = drag.origin.x + cursor.x - drag.cursor.x;
        let y = drag.origin.y + cursor.y - drag.cursor.y;
        let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

pub fn end_drag() {
    let mut drag = drag_state().lock().unwrap_or_else(|err| err.into_inner());
    drag.active = false;
}

fn shares_input(ours: HWND, other: HWND) -> bool {
    let mut current = other;
    for _ in 0..6 {
        if current == ours {
            return true;
        }
        let Ok(owner) = (unsafe { GetWindow(current, GW_OWNER) }) else {
            break;
        };
        if owner == ours || owner.0.is_null() {
            return owner == ours;
        }
        current = owner;
    }
    window_pid(ours) == window_pid(other) || input_popup(other)
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid
}

fn taskbar_or_menu(hwnd: HWND) -> bool {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 {
        return false;
    }
    let name = String::from_utf16_lossy(&buf[..n as usize]);
    matches!(
        name.as_str(),
        "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
            | "#32768"
    )
}

fn input_popup(hwnd: HWND) -> bool {
    let mut buf = [0u16; 96];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 {
        return false;
    }
    let name = String::from_utf16_lossy(&buf[..n as usize]);
    let lower = name.to_ascii_lowercase();
    if lower.contains("ime")
        || lower.contains("msctf")
        || lower.contains("cicero")
        || lower.contains("candidate")
        || lower.contains("inputsite")
    {
        return true;
    }
    name == "Windows.UI.Core.CoreWindow" && text_input_host(hwnd)
}

fn text_input_host(hwnd: HWND) -> bool {
    let pid = window_pid(hwnd);
    if pid == 0 {
        return false;
    }
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return false;
    };
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
    };
    let _ = unsafe { CloseHandle(process) };
    if ok.is_err() {
        return false;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase();
    path.ends_with("textinputhost.exe") || path.ends_with("ctfmon.exe")
}

pub fn centered(width: i32, height: i32) -> (i32, i32) {
    unsafe {
        let mut point = POINT::default();
        if GetCursorPos(&mut point).is_ok() {
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..zeroed()
            };
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                let work = info.rcWork;
                let x = work.left + (work.right - work.left - width).max(0) / 2;
                let y = work.top + ((work.bottom - work.top - height).max(0) as f32 * 0.32) as i32;
                return (x, y);
            }
        }
    }
    (160, 120)
}

pub fn end_folder_session() {
    FOLDER_KEYS.lock().unwrap_or_else(|err| err.into_inner()).clear();
    FOLDER_ARMING.store(false, Ordering::Relaxed);
    FOLDER_SAW_TEXT.store(false, Ordering::Relaxed);
    FOLDER_BACKS.store(0, Ordering::Relaxed);
    FOLDER_SYNC.store(false, Ordering::Relaxed);
    ANCHOR_FREE.store(false, Ordering::Relaxed);
    locate::pin_explorer(0);
    FOLDER_GEN.fetch_add(1, Ordering::Relaxed);
}

pub fn set_folder_synced(on: bool) {
    FOLDER_SYNC.store(on, Ordering::Relaxed);
    if on {
        ANCHOR_FREE.store(false, Ordering::Relaxed);
    }
}

pub fn set_explicit_search(on: bool) {
    EXPLICIT_SEARCH.store(on, Ordering::Relaxed);
}

pub fn explicit_search() -> bool {
    EXPLICIT_SEARCH.load(Ordering::Relaxed)
}

pub fn folder_synced() -> bool {
    FOLDER_SYNC.load(Ordering::Relaxed)
}

pub fn folder_arming() -> bool {
    FOLDER_ARMING.load(Ordering::Relaxed)
}

pub fn folder_saw_text() -> bool {
    FOLDER_SAW_TEXT.load(Ordering::Relaxed)
}

pub fn mark_folder_text() {
    FOLDER_SAW_TEXT.store(true, Ordering::Relaxed);
}

pub fn folder_generation() -> u64 {
    FOLDER_GEN.load(Ordering::Relaxed)
}

pub fn release_anchor() {
    ANCHOR_FREE.store(true, Ordering::Relaxed);
}

pub fn follow_folder(query: &str) {
    if !FOLDER_SYNC.load(Ordering::Relaxed) {
        return;
    }
    if let Some(tx) = LOCATE_TX.get() {
        let _ = tx.send(query.to_string());
    }
}

pub fn explorer_slot(width: i32, height: i32) -> Option<(i32, i32)> {
    if ANCHOR_FREE.load(Ordering::Relaxed) {
        return None;
    }
    let saving = in_save_dialog();
    let list = if saving {
        locate::file_list_hwnd().unwrap_or(0)
    } else {
        0
    };
    let view = if list != 0 {
        list
    } else {
        locate::view_window().unwrap_or(0)
    };
    let raw = if view != 0 {
        view
    } else if saving {
        let pin = locate::pinned_window();
        locate::save_dialog_root(pin).unwrap_or(pin)
    } else {
        locate::pinned_window()
    };
    if raw == 0 {
        return None;
    }
    let explorer = HWND(raw as *mut c_void);
    let mut rect = RECT::default();
    unsafe {
        GetWindowRect(explorer, &mut rect).ok()?;
        let dpi = GetDpiForWindow(explorer).max(96) as i32;
        let margin = if view != 0 { 10 * dpi / 96 } else { 28 * dpi / 96 };
        let (mut x, mut y) = if saving {
            let dialog = save_dialog_frame().unwrap_or(rect);
            // Centered on the dialog's visible bottom. Results grow downward.
            (save_bar_x(dialog, width), dialog.bottom)
        } else {
            (rect.right - width - margin, rect.bottom - height - margin)
        };
        let point = POINT { x: rect.left + 8, y: rect.top + 8 };
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..zeroed()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let work = info.rcWork;
            let right = (work.right - width).max(work.left);
            // The save bar stays on the file list. The work area must not
            // pull it off a small dialog that sits near the screen edge.
            if !(saving && view != 0) {
                x = x.clamp(work.left, right);
                let bottom = (work.bottom - height).max(work.top);
                y = y.clamp(work.top, bottom);
            }
        }
        Some((x, y))
    }
}

pub fn save_bar_width(scale: f32) -> Option<f32> {
    let frame = save_dialog_frame()?;
    let pin = locate::pinned_window();
    let root = locate::save_dialog_root(pin).unwrap_or(pin);
    let hwnd = HWND(root as *mut c_void);
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
    let scale = if scale > 0.0 { scale } else { dpi / 96.0 };
    let dialog = (frame.right - frame.left) as f32 / scale;
    let room = (dialog - 32.0).max(280.0);
    Some(room.min(460.0))
}

pub fn nudge_save_bar(window: &slint::Window, width: i32, height: i32) {
    let Some(hwnd) = pinned_save_hwnd() else {
        return;
    };
    let Some(frame) = client_screen_rect(hwnd) else {
        return;
    };
    move_window(window, save_bar_x(frame, width), frame.bottom, width, height);
}

fn pinned_save_hwnd() -> Option<HWND> {
    let pin = locate::pinned_window();
    if pin == 0 {
        return None;
    }
    let hwnd = HWND(pin as *mut c_void);
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.0.is_null() { hwnd } else { root };
    if !unsafe { IsWindow(Some(root)).as_bool() } {
        return None;
    }
    Some(root)
}

fn save_bar_x(frame: RECT, width: i32) -> i32 {
    let span = frame.right - frame.left;
    frame.left + ((span - width).max(0) / 2)
}

fn save_dialog_frame() -> Option<RECT> {
    let pin = locate::pinned_window();
    if pin == 0 {
        return None;
    }
    let root = locate::save_dialog_root(pin).unwrap_or(pin);
    if root == 0 {
        return None;
    }
    let hwnd = HWND(root as *mut c_void);
    // The client edge is the painted frame. Window rects hang past it by the
    // drop shadow, which is the gap under the dialog border.
    if let Some(visible) = client_screen_rect(hwnd) {
        return Some(visible);
    }
    let mut frame = RECT::default();
    let visible = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
    };
    if visible.is_err() || frame.right <= frame.left || frame.bottom <= frame.top {
        unsafe { GetWindowRect(hwnd, &mut frame) }.ok()?;
    }
    Some(frame)
}

fn client_screen_rect(hwnd: HWND) -> Option<RECT> {
    let mut client = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client) }.ok()?;
    if client.right <= 0 || client.bottom <= 0 {
        return None;
    }
    let mut origin = POINT { x: 0, y: 0 };
    if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
        return None;
    }
    Some(RECT {
        left: origin.x,
        top: origin.y,
        right: origin.x + client.right,
        bottom: origin.y + client.bottom,
    })
}

pub fn save_slot_limit(scale: f32) -> i32 {
    let Some(raw) = locate::file_list_hwnd() else {
        return 9;
    };
    let hwnd = HWND(raw as *mut c_void);
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return 9;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let bottom = locate::filename_row_top()
        .unwrap_or_else(|| save_list_bottom(hwnd, rect.bottom, dpi as i32));
    let px = dpi as f32 / 96.0;
    let scale = if scale > 0.0 { scale } else { px };
    let list_h = (bottom - rect.top) as f32;
    let max_phys = list_h - 2.0 * px;
    let max_logical = max_phys / scale;
    let slots = ((max_logical - 40.0) / 32.0).floor() as i32;
    slots.clamp(1, 9)
}

fn save_list_bottom(list: HWND, fallback: i32, dpi: i32) -> i32 {
    let Some(edit) = locate::filename_edit_hwnd() else {
        return fallback;
    };
    let edit = HWND(edit as *mut c_void);
    let mut name = RECT::default();
    if unsafe { GetWindowRect(edit, &mut name) }.is_err() {
        return fallback;
    }
    // The file list pane is the ancestor that still ends above the filename box.
    // SHELLDLL_DefView often stops at the last file, leaving the bar floating.
    let mut best = fallback;
    let mut current = list;
    for _ in 0..8 {
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(current, &mut rect) }.is_err() {
            break;
        }
        if rect.bottom <= name.top && rect.bottom > best {
            best = rect.bottom;
        }
        let Ok(parent) = (unsafe { GetParent(current) }) else {
            break;
        };
        if parent.0.is_null() || parent == current {
            break;
        }
        current = parent;
    }
    let edge = name.top - 6 * dpi.max(96) / 96;
    if edge > best { edge } else { best }
}

pub fn take_folder_backs() -> u32 {
    FOLDER_BACKS.swap(0, Ordering::Relaxed)
}

pub fn take_folder_text() -> String {
    let mut text = String::new();
    loop {
        let keys = {
            let mut keys = FOLDER_KEYS.lock().unwrap_or_else(|err| err.into_inner());
            std::mem::take(&mut *keys)
        };
        if keys.is_empty() {
            FOLDER_ARMING.store(false, Ordering::Relaxed);
            let extra = {
                let mut keys = FOLDER_KEYS.lock().unwrap_or_else(|err| err.into_inner());
                std::mem::take(&mut *keys)
            };
            if extra.is_empty() {
                return text;
            }
            FOLDER_ARMING.store(true, Ordering::Relaxed);
            push_folder_chars(&mut text, &extra);
            continue;
        }
        push_folder_chars(&mut text, &keys);
    }
}

fn push_folder_chars(text: &mut String, keys: &[u32]) {
    for &vk in keys {
        if let Some(ch) = locate_char(vk) {
            text.push(ch);
        }
    }
}

fn folder_has_keys() -> bool {
    !FOLDER_KEYS.lock().unwrap_or_else(|err| err.into_inner()).is_empty()
}

fn queue_folder_key(vk: u32) -> u64 {
    let hwnd = unsafe { GetForegroundWindow() };
    if !hwnd.0.is_null() {
        let current = hwnd.0 as isize;
        if locate::pinned_window() != current {
            locate::pin_explorer(current);
        }
    }
    FOLDER_ARMING.store(true, Ordering::Relaxed);
    unsafe {
        let _ = AllowSetForegroundWindow(GetCurrentProcessId());
    }
    FOLDER_KEYS.lock().unwrap_or_else(|err| err.into_inner()).push(vk);
    FOLDER_GEN.load(Ordering::Relaxed)
}

fn pop_folder_key() {
    let mut keys = FOLDER_KEYS.lock().unwrap_or_else(|err| err.into_inner());
    keys.pop();
    if keys.is_empty() {
        FOLDER_GEN.fetch_add(1, Ordering::Relaxed);
        FOLDER_ARMING.store(false, Ordering::Relaxed);
        locate::pin_explorer(0);
    }
}

fn cancel_folder_keys() {
    FOLDER_KEYS.lock().unwrap_or_else(|err| err.into_inner()).clear();
    FOLDER_GEN.fetch_add(1, Ordering::Relaxed);
    FOLDER_ARMING.store(false, Ordering::Relaxed);
    locate::pin_explorer(0);
    FOLDER_SYNC.store(false, Ordering::Relaxed);
}

pub fn open_path(path: &str) -> bool {
    if path.to_ascii_lowercase().starts_with("shell:") {
        return Command::new("explorer").arg(path).spawn().is_ok();
    }
    let path = wide(path);
    let operation = wide("open");
    let code = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    code.0 as usize > 32
}

pub fn open_device_manager() -> bool {
    let mut system = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
    system.push("System32");
    let file = wide(&system.join("devmgmt.msc").to_string_lossy());
    let dir = wide(&system.to_string_lossy());
    let operation = wide("open");
    let code = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR(dir.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    code.0 as usize > 32
}

pub fn reveal(path: &str) -> bool {
    Command::new("explorer")
        .arg(format!("/select,{path}"))
        .spawn()
        .is_ok()
}

pub enum ItemMenu {
    Open,
    Reveal,
    Status(String),
}

pub fn cursor_in_window(window: &slint::Window) -> Option<(f32, f32)> {
    let hwnd = native_hwnd(window)?;
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_err() {
        return None;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    let scale = window.scale_factor();
    if scale <= f32::EPSILON {
        return None;
    }
    Some((
        (point.x - rect.left) as f32 / scale,
        (point.y - rect.top) as f32 / scale,
    ))
}

pub fn perform_item(
    window: &slint::Window,
    choice: u32,
    title: &str,
    target: &str,
) -> Option<ItemMenu> {
    let hwnd = native_hwnd(window).unwrap_or(HWND(std::ptr::null_mut()));
    match choice {
        1 => Some(ItemMenu::Open),
        2 => Some(ItemMenu::Reveal),
        3 => Some(ItemMenu::Status(if copy_file(target, false) {
            "已复制，可在文件夹里粘贴".into()
        } else {
            "没有复制".into()
        })),
        4 => Some(ItemMenu::Status(if copy_file(target, true) {
            "已剪切，到目标文件夹粘贴即可移动".into()
        } else {
            "没有剪切".into()
        })),
        5 => Some(ItemMenu::Status(if copy_text(target) {
            "已复制路径".into()
        } else {
            "没有复制路径".into()
        })),
        6 => Some(ItemMenu::Status(if copy_text(title) {
            "已复制名称".into()
        } else {
            "没有复制名称".into()
        })),
        7 => Some(ItemMenu::Status(move_item(hwnd, target))),
        8 => Some(ItemMenu::Status(delete_item(hwnd, target))),
        _ => None,
    }
}

fn copy_text(text: &str) -> bool {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    put_clipboard(&[(13, wide)])
}

fn copy_file(path: &str, cut: bool) -> bool {
    let mut wide: Vec<u16> = path.encode_utf16().collect();
    wide.push(0);
    wide.push(0);
    let mut drop = vec![0u16; 10 + wide.len()];
    drop[0] = 20;
    drop[8] = 1;
    drop[10..].copy_from_slice(&wide);
    let effect = if cut { 2u32 } else { 1u32 };
    let format = unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) };
    put_clipboard(&[(15, drop), (format, vec![effect as u16, (effect >> 16) as u16])])
}

fn put_clipboard(items: &[(u32, Vec<u16>)]) -> bool {
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let mut stored = false;
        for (format, words) in items {
            let Some(mem) = clipboard_memory(words) else {
                continue;
            };
            stored |= SetClipboardData(*format, Some(HANDLE(mem.0))).is_ok();
        }
        let _ = CloseClipboard();
        stored
    }
}

unsafe fn clipboard_memory(words: &[u16]) -> Option<windows::Win32::Foundation::HGLOBAL> {
    let bytes = words.len() * 2;
    let mem = GlobalAlloc(GMEM_MOVEABLE, bytes).ok()?;
    let locked = GlobalLock(mem);
    if locked.is_null() {
        return None;
    }
    std::ptr::copy_nonoverlapping(words.as_ptr(), locked.cast(), words.len());
    let _ = GlobalUnlock(mem);
    Some(mem)
}

fn move_item(hwnd: HWND, path: &str) -> String {
    let Some(folder) = pick_folder(hwnd) else {
        return "已取消".into();
    };
    if file_operation(hwnd, FO_MOVE, path, Some(&folder)) {
        "已移动".into()
    } else {
        "没有移动".into()
    }
}

fn delete_item(hwnd: HWND, path: &str) -> String {
    if file_operation(hwnd, FO_DELETE, path, None) {
        "已删除".into()
    } else {
        "没有删除".into()
    }
}

fn pick_folder(hwnd: HWND) -> Option<String> {
    let mut name = [0u16; 260];
    let title = wide("移动到文件夹");
    let info = BROWSEINFOW {
        hwndOwner: hwnd,
        pidlRoot: std::ptr::null_mut(),
        pszDisplayName: PWSTR(name.as_mut_ptr()),
        lpszTitle: PCWSTR(title.as_ptr()),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE | BIF_EDITBOX,
        lpfn: None,
        lParam: LPARAM(0),
        iImage: 0,
    };
    let pidl = unsafe { SHBrowseForFolderW(&info) };
    if pidl.is_null() {
        return None;
    }
    let mut path = [0u16; 260];
    let ok = unsafe { SHGetPathFromIDListW(pidl, &mut path) }.as_bool();
    unsafe { CoTaskMemFree(Some(pidl.cast())) };
    if !ok {
        return None;
    }
    let end = path.iter().position(|unit| *unit == 0).unwrap_or(path.len());
    Some(String::from_utf16_lossy(&path[..end]))
}

fn file_operation(hwnd: HWND, func: u32, from: &str, to: Option<&str>) -> bool {
    let from_wide = wide_zz(from);
    let to_wide = to.map(wide_zz).unwrap_or_else(|| vec![0]);
    let mut op = SHFILEOPSTRUCTW {
        hwnd,
        wFunc: func,
        pFrom: PCWSTR(from_wide.as_ptr()),
        pTo: PCWSTR(to_wide.as_ptr()),
        fFlags: FOF_ALLOWUNDO.0 as u16,
        fAnyOperationsAborted: false.into(),
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: PCWSTR::null(),
    };
    unsafe { SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool() }
}

fn wide_zz(text: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    wide.push(0);
    wide
}

fn shell_thread() {
    locate::ensure_com();
    load_hotkey();
    load_theme();
    let module = unsafe { GetModuleHandleW(None).unwrap_or_default() };
    let instance = HINSTANCE(module.0);
    let shell_class = register_class(w!("FindthingShell"), instance);
    let pill_class = register_class(w!("FindthingPill"), instance);
    let shell = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            shell_class,
            w!("findthing"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    };
    let Ok(shell) = shell else {
        return;
    };
    TRAY_HWND.store(shell.0 as isize, Ordering::Relaxed);
    let pill = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            pill_class,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    };
    if let Ok(pill) = pill {
        PILL_HWND.store(pill.0 as isize, Ordering::Relaxed);
    }
    SHOW_MSG.store(
        unsafe { RegisterWindowMessageW(w!("Findthing.Show")) },
        Ordering::Relaxed,
    );
    add_tray(shell);
    start_locate_thread();
    refresh_injected_flag();
    sync_registered_hotkey();
    thread::spawn(input_thread);
    thread::spawn(|| {
        let mut seen = 0u32;
        let mut last_focus = String::new();
        loop {
            let (list, editor, focus) = probe_explorer_list();
            EXPLORER_LIST.store(list, Ordering::Relaxed);
            FOCUS_EDITOR.store(editor, Ordering::Relaxed);
            if focus != last_focus {
                note(&format!("focus {focus} list={list} editor={editor}"));
                last_focus = focus;
            }
            let seq = KEY_SEQ.load(Ordering::Relaxed);
            if seq != seen {
                seen = seq;
                note(&format!(
                    "key vk={} act={} list={list} editor={editor}",
                    KEY_VK.load(Ordering::Relaxed),
                    KEY_ACT.load(Ordering::Relaxed)
                ));
            }
            thread::sleep(Duration::from_millis(25));
        }
    });
    unsafe {
        SetTimer(Some(shell), 2, 5000, None);
    }

    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn start_locate_thread() {
    let (tx, rx) = mpsc::channel::<String>();
    let _ = LOCATE_TX.set(tx);
    thread::Builder::new()
        .name("findthing-locate".into())
        .spawn(move || {
            locate::ensure_com();
            while let Ok(mut query) = rx.recv() {
                while let Ok(next) = rx.try_recv() {
                    query = next;
                }
                let generation = GENERATION.load(Ordering::Relaxed);
                if query.is_empty() {
                    continue;
                }
                let found = locate::select_matching(&query);
                if GENERATION.load(Ordering::Relaxed) == generation {
                    post(WM_FOUND, found as usize, generation as isize);
                }
            }
        })
        .ok();
}

fn register_class(name: PCWSTR, instance: HINSTANCE) -> PCWSTR {
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW).unwrap_or_default() },
        lpszClassName: name,
        ..unsafe { zeroed() }
    };
    unsafe {
        RegisterClassExW(&class);
    }
    name
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let show_msg = SHOW_MSG.load(Ordering::Relaxed);
    if show_msg != 0 && message == show_msg {
        LAUNCHER_VISIBLE.store(true, Ordering::Relaxed);
        emit(ShellEvent::Show);
        return LRESULT(0);
    }
    match message {
        WM_TRAY => match lparam.0 as u32 {
            WM_LBUTTONUP => {
                LAUNCHER_VISIBLE.store(true, Ordering::Relaxed);
                emit(ShellEvent::Show);
            }
            WM_RBUTTONDOWN => hold_launcher(1500),
            WM_RBUTTONUP => show_menu(hwnd),
            _ => {}
        },
        WM_COMMAND => match (wparam.0 & 0xffff) as usize {
            1 => {
                LAUNCHER_VISIBLE.store(true, Ordering::Relaxed);
                emit(ShellEvent::Show);
            }
            3 => {
                release_registered_hotkey();
                CAPTURE.store(true, Ordering::Relaxed);
                LAUNCHER_VISIBLE.store(true, Ordering::Relaxed);
                emit(ShellEvent::Capture);
            }
            2 => quit(hwnd),
            4 => set_startup(!startup_enabled()),
            10 => store_theme(Theme::Paper),
            11 => store_theme(Theme::Field),
            12 => store_theme(Theme::Ink),
            13 => store_theme(Theme::Split),
            14 => store_theme(Theme::Ledger),
            _ => {}
        },
        WM_TOGGLE => {
            hide_pill();
            if wparam.0 == 0 {
                emit(ShellEvent::Hide);
            } else {
                emit(ShellEvent::Show);
            }
        }
        WM_HOTKEY => {
            if !CAPTURE.load(Ordering::Relaxed) {
                note("wm_hotkey");
                toggle_launcher();
            }
            return LRESULT(0);
        }
        WM_QUERY => sync_query(),
        WM_FOUND => {
            let current = {
                let pill = PILL.lock().unwrap_or_else(|err| err.into_inner());
                !pill.text.is_empty()
            };
            if current && lparam.0 as u64 == GENERATION.load(Ordering::Relaxed) {
                {
                    let mut pill = PILL.lock().unwrap_or_else(|err| err.into_inner());
                    pill.found = wparam.0 != 0;
                }
                let pill = pill_hwnd();
                if !pill.0.is_null() {
                    let _ = InvalidateRect(Some(pill), None, true);
                }
            }
        }
        WM_TIMER => {
            if wparam.0 == 2 {
                refresh_injected_flag();
                return LRESULT(0);
            }
            clear_query();
            hide_pill();
        }
        WM_PAINT => {
            if hwnd == pill_hwnd() {
                paint_pill(hwnd);
                return LRESULT(0);
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

fn quit(hwnd: HWND) {
    remove_tray(hwnd);
    let hook = HOOK.swap(0, Ordering::Relaxed);
    if hook != 0 {
        unsafe {
            let _ = UnhookWindowsHookEx(HHOOK(hook as *mut c_void));
        }
    }
    emit(ShellEvent::Quit);
    unsafe { PostQuitMessage(0) };
}

fn show_menu(hwnd: HWND) {
    hold_launcher(60_000);
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            hold_launcher(0);
            return;
        };
        let _ = AppendMenuW(menu, MF_STRING, 1, w!("显示搜索"));
        if let Ok(themes) = CreatePopupMenu() {
            let current = *theme_slot();
            let _ = AppendMenuW(themes, theme_flags(current, Theme::Paper), 10, w!("素纸"));
            let _ = AppendMenuW(themes, theme_flags(current, Theme::Field), 11, w!("轻条"));
            let _ = AppendMenuW(themes, theme_flags(current, Theme::Ink), 12, w!("夜墨"));
            let _ = AppendMenuW(themes, theme_flags(current, Theme::Split), 13, w!("双笺"));
            let _ = AppendMenuW(themes, theme_flags(current, Theme::Ledger), 14, w!("细目"));
            let _ = AppendMenuW(menu, MF_STRING | MF_POPUP, themes.0 as usize, w!("窗口外观"));
        }
        let _ = AppendMenuW(menu, MF_STRING, 3, w!("设置呼出快捷键"));
        let startup_flags = if startup_enabled() { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, startup_flags, 4, w!("开机启动"));
        let _ = AppendMenuW(menu, MF_STRING, 2, w!("退出"));
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        let _ = SetForegroundWindow(hwnd);
        let _ = TrackPopupMenu(
            menu,
            TPM_RIGHTALIGN | TPM_BOTTOMALIGN,
            point.x,
            point.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
    }
    hold_launcher(800);
}

fn sync_query() {
    let text = query_text();
    if text.is_empty() {
        hide_pill();
        return;
    }
    show_pill(&text, false);
    let hwnd = tray_hwnd();
    unsafe {
        SetTimer(Some(hwnd), 1, 700, None);
    }
    if let Some(tx) = LOCATE_TX.get() {
        let _ = tx.send(text);
    }
}

fn show_pill(text: &str, found: bool) {
    {
        let mut pill = PILL.lock().unwrap_or_else(|err| err.into_inner());
        pill.text = text.to_string();
        pill.found = found;
    }
    let pill = pill_hwnd();
    let explorer = unsafe { GetForegroundWindow() };
    if pill.0.is_null() || explorer.0.is_null() {
        return;
    }
    let mut rect = RECT::default();
    unsafe {
        if GetWindowRect(explorer, &mut rect).is_err() {
            return;
        }
        let dpi = GetDpiForWindow(explorer).max(96);
        let width = (text.chars().count() as i32 * 14 + 36) * dpi as i32 / 96;
        let height = 34 * dpi as i32 / 96;
        let x = rect.left + (rect.right - rect.left - width) / 2;
        let y = rect.bottom - height - 28 * dpi as i32 / 96;
        let region = CreateRoundRectRgn(0, 0, width, height, height, height);
        if !region.is_invalid() {
            let _ = SetWindowRgn(pill, Some(region), true);
        }
        let _ = SetWindowPos(
            pill,
            Some(HWND_TOPMOST),
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        let _ = ShowWindow(pill, SW_SHOWNOACTIVATE);
        let _ = InvalidateRect(Some(pill), None, true);
    }
}

fn hide_pill() {
    {
        let mut pill = PILL.lock().unwrap_or_else(|err| err.into_inner());
        pill.text.clear();
        pill.found = false;
    }
    let pill = pill_hwnd();
    if !pill.0.is_null() {
        unsafe {
            let _ = ShowWindow(pill, SW_HIDE);
        }
    }
}

fn paint_pill(hwnd: HWND) {
    let (text, found) = {
        let pill = PILL.lock().unwrap_or_else(|err| err.into_inner());
        (wide(&pill.text), pill.found)
    };
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut paint);
        if dc.is_invalid() {
            return;
        }
        let mut client = RECT::default();
        let _ = GetWindowRect(hwnd, &mut client);
        let width = client.right - client.left;
        let height = client.bottom - client.top;
        let mut rect = RECT { left: 0, top: 0, right: width, bottom: height };
        let brush = CreateSolidBrush(COLORREF(0x00e6f0f4));
        if !brush.is_invalid() {
            FillRect(dc, &rect, brush);
            let _ = DeleteObject(brush.into());
        }
        let dpi = GetDpiForWindow(hwnd).max(96);
        let font = CreateFontW(
            -((15 * dpi as i32) / 96),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Microsoft YaHei UI"),
        );
        let old = if font.is_invalid() {
            windows::Win32::Graphics::Gdi::HGDIOBJ::default()
        } else {
            SelectObject(dc, font.into())
        };
        SetBkMode(dc, TRANSPARENT);
        let _ = SetTextColor(dc, COLORREF(0x00332017));
        rect.left = if found { 22 } else { 14 };
        let mut chars = text;
        let _ = DrawTextW(dc, &mut chars, &mut rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        if !old.is_invalid() {
            SelectObject(dc, old);
        }
        if !font.is_invalid() {
            let _ = DeleteObject(font.into());
        }
        if found {
            let dot = CreateSolidBrush(COLORREF(0x00324eef));
            if !dot.is_invalid() {
                let dot_rect = RECT { left: 10, top: height / 2 - 4, right: 18, bottom: height / 2 + 4 };
                FillRect(dc, &dot_rect, dot);
                let _ = DeleteObject(dot.into());
            }
        }
        let _ = EndPaint(hwnd, &paint);
    }
}

fn add_tray(hwnd: HWND) {
    let mut tip = [0u16; 128];
    for (index, unit) in "findthing".encode_utf16().enumerate() {
        tip[index] = unit;
    }
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: brand_icon(32),
        szTip: tip,
        ..unsafe { zeroed() }
    };
    let _ = data.Anonymous;
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &data);
    }
}

fn remove_tray(hwnd: HWND) {
    let data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..unsafe { zeroed() }
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

pub fn apply_window_icon(window: &slint::Window) {
    let Some(hwnd) = native_hwnd(window) else {
        return;
    };
    unsafe {
        let small = brand_icon(16);
        let big = brand_icon(32);
        if !small.0.is_null() {
            let _ = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(0)), Some(LPARAM(small.0 as isize)));
        }
        if !big.0.is_null() {
            let _ = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(1)), Some(LPARAM(big.0 as isize)));
        }
    }
}

fn brand_icon(size: i32) -> windows::Win32::UI::WindowsAndMessaging::HICON {
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        if let Ok(handle) = LoadImageW(
            Some(HINSTANCE(module.0)),
            PCWSTR(1 as *const u16),
            IMAGE_ICON,
            size,
            size,
            LR_SHARED,
        ) {
            if !handle.0.is_null() {
                return windows::Win32::UI::WindowsAndMessaging::HICON(handle.0);
            }
        }
    }
    raster_icon(size)
}

fn raster_icon(size: i32) -> windows::Win32::UI::WindowsAndMessaging::HICON {
    let pixels = raster(size as usize);
    unsafe {
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..zeroed()
            },
            ..zeroed()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return windows::Win32::UI::WindowsAndMessaging::HICON::default();
        };
        if !bits.is_null() {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u32, pixels.len());
        }
        let mask_bits = vec![0u8; ((size as usize + 15) / 16) * 2 * size as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr() as *const c_void));
        if mask.is_invalid() {
            let _ = DeleteObject(color.into());
            return windows::Win32::UI::WindowsAndMessaging::HICON::default();
        }
        let icon_info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&icon_info).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

fn raster(size: usize) -> Vec<u32> {
    let mut pixels = vec![0u32; size * size];
    for y in 0..size {
        for x in 0..size {
            let nx = (x as f32 + 0.5) / size as f32;
            let ny = (y as f32 + 0.5) / size as f32;
            pixels[y * size + x] = icon_pixel(nx, ny);
        }
    }
    pixels
}

fn icon_pixel(x: f32, y: f32) -> u32 {
    if !in_squircle(x, y) {
        return 0;
    }
    let mut pixel = bgra(0x17, 0x20, 0x33, 0xff);
    let dx = x - 0.43;
    let dy = y - 0.41;
    let dist = (dx * dx + dy * dy).sqrt();
    if (0.145..0.205).contains(&dist) || segment_distance(x, y, 0.55, 0.54, 0.76, 0.76) < 0.035 {
        pixel = bgra(0xf4, 0xf0, 0xe6, 0xff);
    }
    if (0.35..0.50).contains(&x) && (0.31..0.52).contains(&y) {
        pixel = bgra(0xef, 0x4e, 0x32, 0xff);
    }
    pixel
}

fn in_squircle(x: f32, y: f32) -> bool {
    let radius = 0.24;
    if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return false;
    }
    let px = x.clamp(radius, 1.0 - radius);
    let py = y.clamp(radius, 1.0 - radius);
    let dx = x - px;
    let dy = y - py;
    dx * dx + dy * dy <= radius * radius
}

fn segment_distance(x: f32, y: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let len = dx * dx + dy * dy;
    let t = if len == 0.0 {
        0.0
    } else {
        (((x - x1) * dx + (y - y1) * dy) / len).clamp(0.0, 1.0)
    };
    let px = x1 + t * dx - x;
    let py = y1 + t * dy - y;
    (px * px + py * py).sqrt()
}

fn bgra(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if handle_key(info) {
            return LRESULT(1);
        }
    }
    let hook = HOOK.load(Ordering::Relaxed);
    CallNextHookEx(Some(HHOOK(hook as *mut c_void)), code, wparam, lparam)
}

fn handle_key(info: &KBDLLHOOKSTRUCT) -> bool {
    if info.dwExtraInfo == REPLAY_MARK {
        return false;
    }
    let injected = info.flags.contains(KBDLLHOOKSTRUCT_FLAGS(0x10));
    if injected {
        let left = REPLAY_LEFT.load(Ordering::SeqCst);
        if left > 0 {
            REPLAY_LEFT.fetch_sub(1, Ordering::SeqCst);
            return false;
        }
    }
    let up = info.flags.contains(KBDLLHOOKSTRUCT_FLAGS(LLKHF_UP));
    if !up {
        mark_key(info.vkCode, 7);
    }
    let vk = event_vk(info);
    let alt = info.flags.contains(KBDLLHOOKSTRUCT_FLAGS(LLKHF_ALTDOWN));
    let repeat = !up && HELD.load(Ordering::Relaxed) == vk;
    if !up {
        HELD.store(vk, Ordering::Relaxed);
    } else if HELD.load(Ordering::Relaxed) == vk {
        HELD.store(0, Ordering::Relaxed);
    }
    track_modifier(vk, up);

    if CAPTURE.load(Ordering::Relaxed) {
        return capture_key(vk, up, info.time, alt);
    }
    if vk == VK_LCONTROL.0 as u32 || vk == VK_RCONTROL.0 as u32 {
        if up {
            on_ctrl_up(info.time);
        }
        return false;
    }
    if !up && !repeat && vk == VK_ESCAPE.0 as u32 && FOLDER_SYNC.load(Ordering::Relaxed) {
        if save_dialog_in_use() {
            return false;
        }
        cancel_folder_keys();
        hide_pill();
        emit(ShellEvent::Hide);
        return true;
    }
    if !up && !repeat && chord_pressed(vk, alt) {
        if HOTKEY_REGISTERED.load(Ordering::Relaxed) {
            return false;
        }
        toggle_launcher();
        return true;
    }
    if !up && modifier_blocks_locate(alt) {
        CTRL_AS_MOD.store(true, Ordering::Relaxed);
        mark_key(vk, 3);
        return false;
    }
    if !up && in_explorer_list() && (FOLDER_ARMING.load(Ordering::Relaxed) || FOLDER_SYNC.load(Ordering::Relaxed)) {
        if vk == VK_ESCAPE.0 as u32 {
            cancel_folder_keys();
            hide_pill();
            emit(ShellEvent::Hide);
            return true;
        }
        if vk == VK_BACK.0 as u32 {
            if folder_has_keys() {
                pop_folder_key();
            } else {
                FOLDER_BACKS.fetch_add(1, Ordering::Relaxed);
                emit(ShellEvent::Folder(folder_generation()));
            }
            return true;
        }
        if let Some(ch) = locate_char(vk) {
            if !(ch == ' ' && !folder_has_keys() && !FOLDER_SYNC.load(Ordering::Relaxed)) {
                let generation = queue_folder_key(vk);
                mark_key(vk, 1);
                emit(ShellEvent::Folder(generation));
                return true;
            }
        }
    }
    if up {
        return false;
    }
    // The main window can stay "visible" after Alt+Space. Typing in the file
    // list still starts locate; keys only reach this point when Explorer is foreground.
    if in_explorer_list() {
        if vk == VK_ESCAPE.0 as u32 {
            if !folder_has_keys() {
                return false;
            }
            cancel_folder_keys();
            hide_pill();
            return true;
        }
        if vk == VK_BACK.0 as u32 {
            if !folder_has_keys() {
                return false;
            }
            pop_folder_key();
            return true;
        }
        if let Some(ch) = locate_char(vk) {
            if ch == ' ' && !folder_has_keys() {
                return false;
            }
            let generation = queue_folder_key(vk);
            mark_key(vk, 1);
            emit(ShellEvent::Folder(generation));
            return true;
        }
        mark_key(vk, 4);
    } else if !up && locate_char(vk).is_some() {
        mark_key(vk, 2);
    }
    false
}

fn on_ctrl_up(time: u32) {
    if CTRL_AS_MOD.swap(false, Ordering::Relaxed) {
        return;
    }
    let previous = LAST_CTRL_UP.swap(time, Ordering::Relaxed);
    if time.wrapping_sub(previous) < 400 {
        LAST_CTRL_UP.store(0, Ordering::Relaxed);
        if CAPTURE.load(Ordering::Relaxed) {
            store_hotkey(Hotkey::DoubleCtrl);
            return;
        }
        if current_hotkey() == Hotkey::DoubleCtrl {
            toggle_launcher();
        }
    }
}

fn modifier_blocks_locate(alt_down: bool) -> bool {
    if alt_down {
        return true;
    }
    let mut held = MOD_HELD.load(Ordering::Relaxed);
    let mut stale = 0u32;
    if held & (HELD_LALT | HELD_RALT) != 0 {
        stale |= HELD_LALT | HELD_RALT;
    }
    if held & (HELD_LCTRL | HELD_RCTRL) != 0 && !key_down(VK_CONTROL.0) {
        stale |= HELD_LCTRL | HELD_RCTRL;
    }
    if held & (HELD_LWIN | HELD_RWIN) != 0 && !key_down(VK_LWIN.0) && !key_down(VK_RWIN.0) {
        stale |= HELD_LWIN | HELD_RWIN;
    }
    if stale != 0 {
        held &= !stale;
        MOD_HELD.fetch_and(!stale, Ordering::Relaxed);
    }
    held & (HELD_LCTRL | HELD_RCTRL | HELD_LWIN | HELD_RWIN) != 0
}

fn mark_key(vk: u32, act: u32) {
    KEY_VK.store(vk, Ordering::Relaxed);
    KEY_ACT.store(act, Ordering::Relaxed);
    KEY_SEQ.fetch_add(1, Ordering::Relaxed);
}

fn toggle_launcher() {
    unsafe {
        let _ = AllowSetForegroundWindow(GetCurrentProcessId());
    }
    clear_query();
    if FOLDER_SYNC.load(Ordering::Relaxed) {
        set_explicit_search(true);
        LAUNCHER_VISIBLE.store(true, Ordering::Relaxed);
        post(WM_TOGGLE, 1, 0);
        return;
    }
    let open = !LAUNCHER_VISIBLE.load(Ordering::Relaxed);
    LAUNCHER_VISIBLE.store(open, Ordering::Relaxed);
    if open {
        set_explicit_search(true);
    }
    post(WM_TOGGLE, open as usize, 0);
}

fn capture_key(vk: u32, up: bool, time: u32, alt_down: bool) -> bool {
    if up {
        if vk == VK_LCONTROL.0 as u32 || vk == VK_RCONTROL.0 as u32 {
            on_ctrl_up(time);
        }
        return false;
    }
    if vk == VK_ESCAPE.0 as u32 {
        CAPTURE.store(false, Ordering::Relaxed);
        sync_registered_hotkey();
        emit(ShellEvent::CaptureDone);
        return true;
    }
    if is_modifier(vk) {
        return false;
    }
    let mods = held_mods(alt_down);
    if mods == 0 {
        return false;
    }
    store_hotkey(Hotkey::Chord(mods, vk));
    true
}

fn chord_pressed(vk: u32, alt_down: bool) -> bool {
    let Hotkey::Chord(mods, key) = current_hotkey() else {
        return false;
    };
    vk == key && held_mods(alt_down) == mods
}

const HELD_LCTRL: u32 = 1 << 0;
const HELD_RCTRL: u32 = 1 << 1;
const HELD_LALT: u32 = 1 << 2;
const HELD_RALT: u32 = 1 << 3;
const HELD_LSHIFT: u32 = 1 << 4;
const HELD_RSHIFT: u32 = 1 << 5;
const HELD_LWIN: u32 = 1 << 6;
const HELD_RWIN: u32 = 1 << 7;

fn track_modifier(vk: u32, up: bool) {
    let bit = if vk == VK_LCONTROL.0 as u32 {
        HELD_LCTRL
    } else if vk == VK_RCONTROL.0 as u32 || vk == VK_CONTROL.0 as u32 {
        HELD_RCTRL
    } else if vk == VK_LMENU.0 as u32 {
        HELD_LALT
    } else if vk == VK_RMENU.0 as u32 || vk == VK_MENU.0 as u32 {
        HELD_RALT
    } else if vk == VK_SHIFT.0 as u32 || vk == 0xA0 {
        HELD_LSHIFT
    } else if vk == 0xA1 {
        HELD_RSHIFT
    } else if vk == VK_LWIN.0 as u32 {
        HELD_LWIN
    } else if vk == VK_RWIN.0 as u32 {
        HELD_RWIN
    } else {
        return;
    };
    if up {
        MOD_HELD.fetch_and(!bit, Ordering::Relaxed);
    } else {
        MOD_HELD.fetch_or(bit, Ordering::Relaxed);
    }
}

fn held_mods(alt_down: bool) -> u32 {
    let held = MOD_HELD.load(Ordering::Relaxed);
    let mut tracked = 0;
    if held & (HELD_LCTRL | HELD_RCTRL) != 0 {
        tracked |= MOD_CTRL;
    }
    if alt_down || held & (HELD_LALT | HELD_RALT) != 0 {
        tracked |= MOD_ALT;
    }
    if held & (HELD_LSHIFT | HELD_RSHIFT) != 0 {
        tracked |= MOD_SHIFT;
    }
    if held & (HELD_LWIN | HELD_RWIN) != 0 {
        tracked |= MOD_WIN;
    }
    if tracked != 0 {
        return tracked;
    }
    live_mods()
}

fn release_registered_hotkey() {
    let hwnd = tray_hwnd();
    if hwnd.0.is_null() {
        return;
    }
    unsafe {
        UnregisterHotKey(hwnd.0, HOTKEY_ID);
    }
    HOTKEY_REGISTERED.store(false, Ordering::Relaxed);
}

fn sync_registered_hotkey() {
    let hwnd = tray_hwnd();
    if hwnd.0.is_null() {
        return;
    }
    unsafe {
        UnregisterHotKey(hwnd.0, HOTKEY_ID);
    }
    let Hotkey::Chord(mods, vk) = current_hotkey() else {
        HOTKEY_REGISTERED.store(false, Ordering::Relaxed);
        note("hotkey is double-ctrl");
        return;
    };
    let mut win = WIN_MOD_NOREPEAT;
    if mods & MOD_ALT != 0 {
        win |= WIN_MOD_ALT;
    }
    if mods & MOD_CTRL != 0 {
        win |= WIN_MOD_CONTROL;
    }
    if mods & MOD_SHIFT != 0 {
        win |= WIN_MOD_SHIFT;
    }
    if mods & MOD_WIN != 0 {
        win |= WIN_MOD_WIN;
    }
    let ok = unsafe { RegisterHotKey(hwnd.0, HOTKEY_ID, win, vk) };
    if ok != 0 {
        HOTKEY_REGISTERED.store(true, Ordering::Relaxed);
        note(&format!("registered mods={mods} vk={vk}"));
    } else {
        HOTKEY_REGISTERED.store(false, Ordering::Relaxed);
        let err = unsafe { GetLastError() };
        note(&format!("register failed mods={mods} vk={vk} err={}", err.0));
    }
}

fn note(line: &str) {
    use std::io::Write;
    let mut path = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    path.push("findthing");
    let _ = std::fs::create_dir_all(&path);
    path.push("hotkey.log");
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{line}");
}

fn install_hook() {
    let module = unsafe { GetModuleHandleW(None).unwrap_or_default() };
    let instance = HINSTANCE(module.0);
    if HOOK.load(Ordering::Relaxed) == 0 {
        if let Ok(hook) =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), Some(instance), 0) }
        {
            HOOK.store(hook.0 as isize, Ordering::Relaxed);
            note("hook on");
        }
    }
    if MOUSE_HOOK.load(Ordering::Relaxed) == 0 {
        if let Ok(hook) =
            unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(instance), 0) }
        {
            MOUSE_HOOK.store(hook.0 as isize, Ordering::Relaxed);
        }
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam.0 as u32;
        // Left, right, middle, and extra button presses. Wheel movement stays.
        if msg == 0x0201 || msg == 0x0204 || msg == 0x0207 || msg == 0x020B {
            let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            if click_should_dismiss(info.pt) {
                emit(ShellEvent::Hide);
            }
        }
    }
    let hook = MOUSE_HOOK.load(Ordering::Relaxed);
    CallNextHookEx(Some(HHOOK(hook as *mut c_void)), code, wparam, lparam)
}

fn click_should_dismiss(pt: POINT) -> bool {
    if !LAUNCHER_VISIBLE.load(Ordering::Relaxed) {
        return false;
    }
    if FOLDER_SYNC.load(Ordering::Relaxed) {
        return dock_click_outside(pt);
    }
    // The window is often shown without becoming the foreground window, so a
    // focus poll never sees the first outside click. The click position does.
    point_outside_launcher(pt)
}

fn point_outside_launcher(pt: POINT) -> bool {
    let raw = LAUNCHER_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return false;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd_from(raw), &mut rect) }.is_err() {
        return false;
    }
    pt.x < rect.left || pt.x >= rect.right || pt.y < rect.top || pt.y >= rect.bottom
}

fn dock_click_outside(pt: POINT) -> bool {
    if !FOLDER_SYNC.load(Ordering::Relaxed) || !LAUNCHER_VISIBLE.load(Ordering::Relaxed) {
        return false;
    }
    let raw = LAUNCHER_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return true;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd_from(raw), &mut rect) }.is_err() {
        return true;
    }
    let inside = pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom;
    if inside {
        return false;
    }
    click_in_pinned_save(pt) == false
}

fn click_in_pinned_save(pt: POINT) -> bool {
    let pin = locate::pinned_window();
    let tracked = SAVE_TRACKED.load(Ordering::Relaxed);
    let root = locate::save_dialog_root(pin)
        .or_else(|| locate::save_dialog_root(tracked))
        .or_else(|| (tracked != 0).then_some(tracked));
    let Some(root) = root else {
        return false;
    };
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd_from(root), &mut rect) }.is_err() {
        return false;
    }
    pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom
}

fn input_thread() {
    unsafe extern "system" fn on_window(
        _hook: *mut c_void,
        _event: u32,
        hwnd: HWND,
        _object: i32,
        _child: i32,
        _thread: u32,
        _time: u32,
    ) {
        remember_explorer(hwnd);
    }
    install_hook();
    remember_explorer(unsafe { GetForegroundWindow() });
    let _event = unsafe {
        SetWinEventHook(
            0x0003,
            0x0003,
            std::ptr::null_mut(),
            on_window,
            0,
            0,
            0x0002,
        )
    };
    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, None, 0, 0).0 > 0 } {
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

static SAVE_TRACKED: AtomicIsize = AtomicIsize::new(0);
static SAVE_RETRY: AtomicBool = AtomicBool::new(false);

fn remember_explorer(hwnd: HWND) {
    if hwnd.0.is_null() {
        return;
    }
    let class = class_name(hwnd);
    if class == "CabinetWClass" || class == "ExploreWClass" {
        let raw = hwnd.0 as isize;
        if KNOWN_EXPLORER.swap(raw, Ordering::Relaxed) != raw {
            EXPLORER_LIST.store(true, Ordering::Relaxed);
            note(&format!("seen window {raw}"));
        }
        leave_save_dialog();
        return;
    }
    if KNOWN_EXPLORER.load(Ordering::Relaxed) != 0 {
        EXPLORER_LIST.store(false, Ordering::Relaxed);
    }
    note_save_window(hwnd);
}

fn note_save_window(hwnd: HWND) {
    if let Some(root) = locate::save_dialog_root(hwnd.0 as isize) {
        show_save_dialog(root);
        return;
    }
    if !locate::save_titled(hwnd.0 as isize) {
        leave_save_dialog();
        return;
    }
    if SAVE_RETRY.swap(true, Ordering::Relaxed) {
        return;
    }
    thread::spawn(|| {
        for _ in 0..10 {
            thread::sleep(Duration::from_millis(120));
            let fg = unsafe { GetForegroundWindow() };
            if let Some(root) = locate::save_dialog_root(fg.0 as isize) {
                show_save_dialog(root);
                SAVE_RETRY.store(false, Ordering::Relaxed);
                return;
            }
            if !locate::save_titled(fg.0 as isize) {
                break;
            }
        }
        SAVE_RETRY.store(false, Ordering::Relaxed);
    });
}

fn show_save_dialog(root: isize) {
    if SAVE_TRACKED.swap(root, Ordering::Relaxed) != root {
        locate::pin_explorer(root);
        emit(ShellEvent::Save);
    }
}

fn leave_save_dialog() {
    if !foreground_left_save() || EXPLICIT_SEARCH.load(Ordering::Relaxed) {
        return;
    }
    SAVE_TRACKED.store(0, Ordering::Relaxed);
    emit(ShellEvent::Hide);
}

fn foreground_left_save() -> bool {
    let tracked = SAVE_TRACKED.load(Ordering::Relaxed);
    if tracked == 0 {
        return false;
    }
    let fg = unsafe { GetForegroundWindow() };
    if fg.0.is_null() {
        return false;
    }
    if locate::save_dialog_root(fg.0 as isize) == Some(tracked) || locate::save_titled(fg.0 as isize) {
        return false;
    }
    if let Ok(owner) = unsafe { GetWindow(fg, GW_OWNER) } {
        if !owner.0.is_null() && owner.0 as isize == tracked {
            return false;
        }
    }
    let mut pid = 0u32;
    let _ = unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    pid != std::process::id()
}

pub fn save_dialog_in_use() -> bool {
    if EXPLICIT_SEARCH.load(Ordering::Relaxed) {
        return false;
    }
    let fg = unsafe { GetForegroundWindow() };
    if fg.0.is_null() {
        return false;
    }
    let tracked = SAVE_TRACKED.load(Ordering::Relaxed);
    let root = if tracked != 0 {
        hwnd_from(tracked)
    } else {
        save_root(fg)
    };
    if root.0.is_null()
        || !unsafe { IsWindow(Some(root)).as_bool() }
        || unsafe { IsIconic(root).as_bool() }
        || !unsafe { IsWindowVisible(root).as_bool() }
    {
        return false;
    }
    if fg == root || save_root(fg) == root {
        return true;
    }
    if let Ok(owner) = unsafe { GetWindow(fg, GW_OWNER) } {
        if !owner.0.is_null() && (owner == root || save_root(owner) == root) {
            return true;
        }
    }
    let mut pid = 0u32;
    let _ = unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    pid == std::process::id()
}

pub fn keep_save_bar() {
    if !save_dialog_in_use() {
        return;
    }
    let fg = unsafe { GetForegroundWindow() };
    let tracked = SAVE_TRACKED.load(Ordering::Relaxed);
    let root = if tracked != 0 {
        hwnd_from(tracked)
    } else {
        save_root(fg)
    };
    if root.0.is_null() {
        return;
    }
    SAVE_TRACKED.store(root.0 as isize, Ordering::Relaxed);
    locate::pin_explorer(root.0 as isize);
    if !LAUNCHER_VISIBLE.load(Ordering::Relaxed) {
        emit(ShellEvent::Save);
    }
}

fn refresh_injected_flag() {
    ACCEPT_INJECTED.store(
        std::path::Path::new(r"G:\findthing\target\accept-injected").exists(),
        Ordering::Relaxed,
    );
}

fn is_modifier(vk: u32) -> bool {
    vk == VK_LCONTROL.0 as u32
        || vk == VK_RCONTROL.0 as u32
        || vk == VK_MENU.0 as u32
        || vk == VK_LMENU.0 as u32
        || vk == VK_RMENU.0 as u32
        || vk == VK_SHIFT.0 as u32
        || vk == 0xA0
        || vk == 0xA1
        || vk == VK_LWIN.0 as u32
        || vk == VK_RWIN.0 as u32
}

fn live_mods() -> u32 {
    let mut mods = 0;
    if key_down(VK_CONTROL.0) {
        mods |= MOD_CTRL;
    }
    if key_down(VK_MENU.0) || key_down(VK_LMENU.0) || key_down(VK_RMENU.0) {
        mods |= MOD_ALT;
    }
    if key_down(VK_SHIFT.0) {
        mods |= MOD_SHIFT;
    }
    if key_down(VK_LWIN.0) || key_down(VK_RWIN.0) {
        mods |= MOD_WIN;
    }
    mods
}

fn native_hwnd(window: &slint::Window) -> Option<HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle_slot = window.window_handle();
    let handle = handle_slot.window_handle().ok()?;
    let RawWindowHandle::Win32(win) = handle.as_raw() else {
        return None;
    };
    let hwnd = HWND(isize::from(win.hwnd) as *mut c_void);
    if hwnd.0.is_null() { None } else { Some(hwnd) }
}

fn current_hotkey() -> Hotkey {
    *HOTKEY.lock().unwrap_or_else(|err| err.into_inner())
}

fn store_hotkey(hotkey: Hotkey) {
    *HOTKEY.lock().unwrap_or_else(|err| err.into_inner()) = hotkey;
    CAPTURE.store(false, Ordering::Relaxed);
    let _ = std::fs::write(hotkey_path(), encode_hotkey(hotkey));
    sync_registered_hotkey();
    emit(ShellEvent::CaptureDone);
}

fn load_hotkey() {
    let Ok(text) = std::fs::read_to_string(hotkey_path()) else {
        return;
    };
    if let Some(hotkey) = parse_hotkey(text.trim()) {
        *HOTKEY.lock().unwrap_or_else(|err| err.into_inner()) = hotkey;
    }
}

fn theme_slot() -> std::sync::MutexGuard<'static, Theme> {
    THEME.lock().unwrap_or_else(|err| err.into_inner())
}

fn theme_flags(current: Theme, item: Theme) -> windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS {
    if current == item { MF_STRING | MF_CHECKED } else { MF_STRING }
}

fn theme_id(theme: Theme) -> &'static str {
    match theme {
        Theme::Paper => "paper",
        Theme::Field => "field",
        Theme::Ink => "ink",
        Theme::Split => "split",
        Theme::Ledger => "ledger",
    }
}

fn parse_theme(text: &str) -> Theme {
    match text {
        "field" => Theme::Field,
        "ink" => Theme::Ink,
        "split" => Theme::Split,
        "ledger" => Theme::Ledger,
        _ => Theme::Paper,
    }
}

fn store_theme(theme: Theme) {
    *theme_slot() = theme;
    let _ = std::fs::write(theme_path(), theme_id(theme));
    emit(ShellEvent::Theme);
}

fn theme_path() -> std::path::PathBuf {
    let mut path = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    path.push("findthing");
    let _ = std::fs::create_dir_all(&path);
    path.push("theme.txt");
    path
}

fn hotkey_path() -> std::path::PathBuf {
    let mut path = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    path.push("findthing");
    let _ = std::fs::create_dir_all(&path);
    path.push("hotkey.txt");
    path
}

fn encode_hotkey(hotkey: Hotkey) -> String {
    match hotkey {
        Hotkey::DoubleCtrl => "double-ctrl".to_string(),
        Hotkey::Chord(mods, vk) => format!("chord {mods} {vk}"),
    }
}

fn parse_hotkey(text: &str) -> Option<Hotkey> {
    if text == "double-ctrl" {
        return Some(Hotkey::DoubleCtrl);
    }
    let mut parts = text.split_whitespace();
    if parts.next()? != "chord" {
        return None;
    }
    let mods = parts.next()?.parse().ok()?;
    let vk = parts.next()?.parse().ok()?;
    Some(Hotkey::Chord(mods, vk))
}

fn chord_parts(mods: u32, vk: u32) -> Vec<String> {
    let mut parts = Vec::new();
    if mods & MOD_CTRL != 0 {
        parts.push("Ctrl".to_string());
    }
    if mods & MOD_ALT != 0 {
        parts.push("Alt".to_string());
    }
    if mods & MOD_SHIFT != 0 {
        parts.push("Shift".to_string());
    }
    if mods & MOD_WIN != 0 {
        parts.push("Win".to_string());
    }
    let key = match vk {
        0x20 => "Space".to_string(),
        0x09 => "Tab".to_string(),
        0x0D => "Enter".to_string(),
        0x1B => "Esc".to_string(),
        0x41..=0x5A => char::from(b'A' + (vk - 0x41) as u8).to_string(),
        0x70..=0x7B => format!("F{}", vk - 0x6F),
        other => other.to_string(),
    };
    parts.push(key);
    parts
}

fn event_vk(info: &KBDLLHOOKSTRUCT) -> u32 {
    let vk = info.vkCode;
    // Chinese IME reports VK_PROCESSKEY / VK_PACKET here. The scan code is still the physical key.
    if vk == 0xE5 || vk == 0xE7 {
        let mapped = unsafe { MapVirtualKeyW(info.scanCode, MAPVK_VSC_TO_VK_EX) };
        if mapped != 0 {
            return mapped;
        }
    }
    vk
}

fn locate_char(vk: u32) -> Option<char> {
    match vk {
        0x41..=0x5A => Some((b'a' + (vk - 0x41) as u8) as char),
        0x30..=0x39 => Some((b'0' + (vk - 0x30) as u8) as char),
        0xbd => Some(if key_down(VK_SHIFT.0) { '_' } else { '-' }),
        0xbe => Some('.'),
        value if value == VK_SPACE.0 as u32 => Some(' '),
        _ => None,
    }
}

fn key_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) & 0x8000u16 as i16 != 0 }
}

fn in_explorer_list() -> bool {
    if FOCUS_EDITOR.load(Ordering::Relaxed) {
        return false;
    }
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }
        let mut pid = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id() {
            return false;
        }
        if hwnd.0 as isize == KNOWN_EXPLORER.load(Ordering::Relaxed) {
            return true;
        }
        let mut buffer = [0u16; 32];
        let length = GetClassNameW(hwnd, &mut buffer);
        if length > 0 {
            let name = String::from_utf16_lossy(&buffer[..length as usize]);
            if name == "CabinetWClass" || name == "ExploreWClass" {
                remember_explorer(hwnd);
                return true;
            }
        }
        if in_save_dialog_list(hwnd) {
            return true;
        }
    }
    false
}

fn in_save_dialog_list(foreground: HWND) -> bool {
    let root = save_root(foreground);
    if root.0.is_null() {
        return false;
    }
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..zeroed()
        };
        if GetGUIThreadInfo(0, &mut info).is_ok() && !info.hwndFocus.0.is_null() && is_editor(&class_name(info.hwndFocus))
        {
            return false;
        }
    }
    true
}

pub fn in_save_dialog() -> bool {
    let raw = locate::pinned_window();
    let hwnd = if raw != 0 {
        hwnd_from(raw)
    } else {
        unsafe { GetForegroundWindow() }
    };
    !save_root(hwnd).0.is_null()
}

fn save_root(hwnd: HWND) -> HWND {
    if hwnd.0.is_null() {
        return hwnd;
    }
    locate::save_dialog_root(hwnd.0 as isize)
        .map(hwnd_from)
        .unwrap_or(HWND(std::ptr::null_mut()))
}

fn probe_explorer_list() -> (bool, bool, String) {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.0.is_null() {
            return (false, false, "none".to_string());
        }
        let mut pid = 0u32;
        let _thread = GetWindowThreadProcessId(foreground, Some(&mut pid));
        if pid == std::process::id() {
            return (false, false, "findthing".to_string());
        }
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..zeroed()
        };
        let info_ok = GetGUIThreadInfo(0, &mut info).is_ok();
        let focus = if info_ok && !info.hwndFocus.0.is_null() {
            info.hwndFocus
        } else if info_ok && !info.hwndCaret.0.is_null() {
            info.hwndCaret
        } else {
            HWND(std::ptr::null_mut())
        };
        let fg_name = class_name(foreground);
        let focus_name = if focus.0.is_null() {
            String::new()
        } else {
            class_name(focus)
        };
        let in_folder = !explorer_frame(foreground).0.is_null()
            || !explorer_frame(focus).0.is_null()
            || (info_ok && !explorer_frame(info.hwndCaret).0.is_null())
            || explorer_process(pid)
            || fg_name == "CabinetWClass"
            || fg_name == "ExploreWClass";
        let editor = in_folder && !focus_name.is_empty() && is_editor(&focus_name);
        let label = format!("fg={fg_name} focus={focus_name}");
        (!editor && in_folder, editor, label)
    }
}

fn explorer_frame(hwnd: HWND) -> HWND {
    if hwnd.0.is_null() {
        return hwnd;
    }
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.0.is_null() { hwnd } else { root };
    let class = class_name(root);
    if class == "CabinetWClass" || class == "ExploreWClass" {
        root
    } else {
        HWND(std::ptr::null_mut())
    }
}

fn explorer_process(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return false;
    };
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
    };
    let _ = unsafe { CloseHandle(process) };
    if ok.is_err() {
        return false;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase();
    path.ends_with("explorer.exe")
}

fn is_editor(name: &str) -> bool {
    name == "Edit"
        || name.starts_with("RichEdit")
        || name == "ComboBox"
        || name == "ComboBoxEx32"
        || name.contains("Search")
        || name.contains("Address")
        || name.contains("Breadcrumb")
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 96];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if length <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}

fn query_text() -> String {
    QUERY.lock().unwrap_or_else(|err| err.into_inner()).clone()
}

fn clear_query() {
    QUERY.lock().unwrap_or_else(|err| err.into_inner()).clear();
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

fn post(message: u32, wparam: usize, lparam: isize) {
    let hwnd = tray_hwnd();
    if hwnd.0.is_null() {
        return;
    }
    unsafe {
        let _ = PostMessageW(Some(hwnd), message, WPARAM(wparam), LPARAM(lparam));
    }
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "findthing";

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(key: isize, sub: *const u16, options: u32, sam: u32, result: *mut isize) -> i32;
    fn RegCloseKey(key: isize) -> i32;
    fn RegQueryValueExW(key: isize, name: *const u16, reserved: *mut u32, kind: *mut u32, data: *mut u8, len: *mut u32) -> i32;
    fn RegSetValueExW(key: isize, name: *const u16, reserved: u32, kind: u32, data: *const u8, len: u32) -> i32;
    fn RegDeleteValueW(key: isize, name: *const u16) -> i32;
}

fn with_run_key<T>(access: u32, f: impl FnOnce(isize) -> T) -> Option<T> {
    let sub = wide(RUN_KEY);
    let mut key = 0isize;
    let opened = unsafe { RegOpenKeyExW(0x8000_0001u32 as i32 as isize, sub.as_ptr(), 0, access, &mut key) };
    if opened != 0 {
        return None;
    }
    let value = f(key);
    unsafe {
        RegCloseKey(key);
    }
    Some(value)
}

fn startup_enabled() -> bool {
    let name = wide(RUN_VALUE);
    with_run_key(0x0001, |key| {
        let mut kind = 0u32;
        let mut len = 0u32;
        unsafe { RegQueryValueExW(key, name.as_ptr(), std::ptr::null_mut(), &mut kind, std::ptr::null_mut(), &mut len) == 0 && len > 2 }
    })
    .unwrap_or(false)
}

fn set_startup(enabled: bool) {
    let name = wide(RUN_VALUE);
    let _ = with_run_key(0x0002, |key| {
        if enabled {
            let Ok(exe) = std::env::current_exe() else {
                return;
            };
            let data = wide(&format!("\"{}\" --startup", exe.display()));
            unsafe {
                RegSetValueExW(key, name.as_ptr(), 0, 1, data.as_ptr().cast(), (data.len() * 2) as u32);
            }
        } else {
            unsafe {
                RegDeleteValueW(key, name.as_ptr());
            }
        }
    });
}

pub fn refresh_startup_path() {
    if startup_enabled() {
        set_startup(true);
    }
}

pub fn mark_quiet_launch() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or(0);
    let _ = std::fs::write(quiet_path(), stamp.to_string());
}

pub fn take_quiet_launch() -> bool {
    let path = quiet_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let _ = std::fs::remove_file(&path);
    let Ok(stamp) = text.trim().parse::<u64>() else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or(0);
    now.saturating_sub(stamp) < 120
}

fn quiet_path() -> std::path::PathBuf {
    let mut path = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    path.push("findthing");
    let _ = std::fs::create_dir_all(&path);
    path.push("quiet.start");
    path
}

fn emit(event: ShellEvent) {
    if let Some(dispatch) = DISPATCH.get() {
        dispatch(event);
    }
}

fn tray_hwnd() -> HWND {
    hwnd_from(TRAY_HWND.load(Ordering::Relaxed))
}

fn pill_hwnd() -> HWND {
    hwnd_from(PILL_HWND.load(Ordering::Relaxed))
}

fn hwnd_from(value: isize) -> HWND {
    HWND(value as *mut c_void)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::{set_startup, startup_enabled};

    #[test]
    fn the_startup_switch_can_be_turned_on_and_off() {
        let previous = startup_enabled();
        set_startup(true);
        assert!(startup_enabled());
        set_startup(false);
        assert!(!startup_enabled());
        if previous {
            set_startup(true);
        }
    }

    #[test]
    fn a_startup_launch_stays_quiet_once() {
        super::mark_quiet_launch();
        assert!(super::take_quiet_launch());
        assert!(!super::take_quiet_launch());
    }
}
