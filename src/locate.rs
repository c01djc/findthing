use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IServiceProvider, CLSCTX_ALL,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::{VARIANT, VT_I4};
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, STRRET};
use windows::Win32::UI::Shell::{
    IFolderView, IPersistFolder2, IShellBrowser, IShellFolder, IShellView, IShellWindows,
    IWebBrowser2, IWebBrowserApp, SHGetNameFromIDList, ShellWindows,
    StrRetToBufW, SID_STopLevelBrowser, SIGDN_FILESYSPATH, SVGIO_ALLVIEW, SVSI_DESELECTOTHERS,
    SVSI_ENSUREVISIBLE, SVSI_KEYBOARDSELECT, SVSI_NOTAKEFOCUS, SHGDN_INFOLDER,
};
use windows::Win32::System::Ole::IOleWindow;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetAncestor, GetClassNameW, GetCursorPos, GetDlgCtrlID, GetForegroundWindow,
    GetParent, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, SendMessageW, SetCursorPos,
    SetForegroundWindow, GA_ROOT, WM_COMMAND, WM_SETTEXT,
};

const NAME_LIMIT: i32 = 12_000;

pub fn ensure_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

struct Cache {
    key: String,
    names: Vec<(i32, String)>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            key: String::new(),
            names: Vec::new(),
        }
    }
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

static PIN: AtomicIsize = AtomicIsize::new(0);

pub fn pin_explorer(hwnd: isize) {
    PIN.store(hwnd, Ordering::Relaxed);
}

pub fn pinned_window() -> isize {
    PIN.load(Ordering::Relaxed)
}

pub fn foreground_folder() -> Option<PathBuf> {
    let (_, path) = active_folder()?;
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

pub fn select_matching(query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return false;
    }
    let Some((view, folder)) = active_folder() else {
        return false;
    };
    let Some(index) = find_index(&view, &folder, query) else {
        return false;
    };
    select_view_item(&view, index)
}

pub fn select_item(index: i32) -> bool {
    let Some((view, _)) = active_folder() else {
        return false;
    };
    select_view_item(&view, index)
}

pub fn folder_matches(query: &str) -> Vec<(i32, String)> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let Some((view, folder)) = active_folder() else {
        return Vec::new();
    };
    with_names(&view, &folder, |names| {
        let mut prefix = Vec::new();
        let mut rest = Vec::new();
        for (index, name) in names {
            if prefix.len() < 80 && starts_with_ignore(name, query) {
                prefix.push((*index, name.clone()));
            } else if rest.len() < 80 && contains_ignore(name, query) {
                rest.push((*index, name.clone()));
            }
            if prefix.len() == 80 {
                break;
            }
        }
        for item in rest {
            if prefix.len() == 80 {
                break;
            }
            prefix.push(item);
        }
        prefix
    })
}

pub fn file_list_hwnd() -> Option<isize> {
    let pin = pinned_window();
    if pin == 0 {
        return None;
    }
    let root = save_dialog_root(pin).unwrap_or(pin);
    find_shell_view(HWND(root as *mut std::ffi::c_void)).map(|hwnd| hwnd.0 as isize)
}

pub fn view_window() -> Option<isize> {
    if let Some((view, _)) = active_folder() {
        if let Ok(window) = view.cast::<IOleWindow>() {
            if let Ok(hwnd) = unsafe { window.GetWindow() } {
                if !hwnd.0.is_null() {
                    return Some(hwnd.0 as isize);
                }
            }
        }
    }
    let pinned = PIN.load(Ordering::Relaxed);
    if pinned == 0 {
        return None;
    }
    find_shell_view(HWND(pinned as *mut std::ffi::c_void)).map(|hwnd| hwnd.0 as isize)
}

pub fn save_titled(raw: isize) -> bool {
    if raw == 0 {
        return false;
    }
    let hwnd = HWND(raw as *mut std::ffi::c_void);
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.0.is_null() { hwnd } else { root };
    is_save_title(&window_title(root))
}

pub fn save_dialog_root(raw: isize) -> Option<isize> {
    if raw == 0 {
        return None;
    }
    let hwnd = HWND(raw as *mut std::ffi::c_void);
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.0.is_null() { hwnd } else { root };
    if !is_save_title(&window_title(root)) {
        return None;
    }
    find_shell_view(root)?;
    Some(root.0 as isize)
}

pub fn browse_pinned(path: &str) -> bool {
    let path = path.trim();
    if path.is_empty() || !std::path::Path::new(path).is_dir() {
        return false;
    }
    let pinned = PIN.load(Ordering::Relaxed);
    if pinned == 0 {
        return false;
    }
    let root_raw = save_dialog_root(pinned).unwrap_or(pinned);
    let root = HWND(root_raw as *mut std::ffi::c_void);
    let Some(edit) = filename_edit(root) else {
        return false;
    };
    // A trailing slash makes Save treat the text as a folder to open.
    let mut text = path.to_string();
    if !text.ends_with('\\') && !text.ends_with('/') {
        text.push('\\');
    }
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetForegroundWindow(root);
        let target = GetWindowThreadProcessId(root, None);
        let current = GetCurrentThreadId();
        let attached = target != 0 && target != current && AttachThreadInput(current, target, true).as_bool();
        let _ = SetFocus(Some(edit));
        if attached {
            let _ = AttachThreadInput(current, target, false);
        }
        let _ = SendMessageW(edit, WM_SETTEXT, None, Some(LPARAM(wide.as_ptr() as isize)));
        let id = GetDlgCtrlID(edit);
        if let Ok(parent) = GetParent(edit) {
            let notify = ((0x0300u32 as usize) << 16) | (id as u16 as usize);
            let _ = SendMessageW(
                parent,
                WM_COMMAND,
                Some(WPARAM(notify)),
                Some(LPARAM(edit.0 as isize)),
            );
        }
        let _ = SendMessageW(root, WM_COMMAND, Some(WPARAM(1)), None);
    }
    true
}

pub fn filename_holds(path: &str) -> bool {
    let pinned = PIN.load(Ordering::Relaxed);
    if pinned == 0 {
        return false;
    }
    let root_raw = save_dialog_root(pinned).unwrap_or(pinned);
    let root = HWND(root_raw as *mut std::ffi::c_void);
    let Some(edit) = filename_edit(root) else {
        return false;
    };
    let have = window_title(edit);
    let want = path.trim().trim_end_matches(['\\', '/']);
    !want.is_empty() && have.to_lowercase().contains(&want.to_lowercase())
}

pub fn filename_edit_hwnd() -> Option<isize> {
    let pin = pinned_window();
    if pin == 0 {
        return None;
    }
    let root = save_dialog_root(pin).unwrap_or(pin);
    filename_edit(HWND(root as *mut std::ffi::c_void)).map(|hwnd| hwnd.0 as isize)
}

/// Top of the filename row in a save dialog. That row sits just under the file list's bottom border.
pub fn filename_row_top() -> Option<i32> {
    let pin = pinned_window();
    if pin == 0 {
        return None;
    }
    let root_raw = save_dialog_root(pin).unwrap_or(pin);
    let root = HWND(root_raw as *mut std::ffi::c_void);
    let mut dialog = RECT::default();
    if unsafe { GetWindowRect(root, &mut dialog) }.is_err() {
        return None;
    }
    let band = dialog.top + (dialog.bottom - dialog.top) * 2 / 5;
    let mut best: Option<i32> = None;
    scan_filename_row(root, band, &mut best);
    best
}

fn scan_filename_row(parent: HWND, band: i32, best: &mut Option<i32>) {
    let mut after: Option<HWND> = None;
    loop {
        let Ok(hwnd) = (unsafe { FindWindowExW(Some(parent), after, PCWSTR::null(), PCWSTR::null()) })
        else {
            return;
        };
        let class = class_name(hwnd);
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok() {
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            let field = class == "Edit" || class == "ComboBox" || class == "ComboBoxEx32";
            if field && rect.top >= band && width >= 180 && (16..=72).contains(&height) {
                if best.is_none_or(|top| rect.top < top) {
                    *best = Some(rect.top);
                }
            }
        }
        scan_filename_row(hwnd, band, best);
        after = Some(hwnd);
    }
}

fn filename_edit(root: HWND) -> Option<HWND> {
    let mut best: Option<(i32, HWND)> = None;
    walk_filename_edit(root, &mut best);
    best.map(|(_, hwnd)| hwnd)
}

fn walk_filename_edit(parent: HWND, best: &mut Option<(i32, HWND)>) {
    let mut after: Option<HWND> = None;
    loop {
        let Ok(hwnd) = (unsafe { FindWindowExW(Some(parent), after, PCWSTR::null(), PCWSTR::null()) })
        else {
            return;
        };
        if class_name(hwnd) == "Edit" && unsafe { GetDlgCtrlID(hwnd) } == 1001 {
            let mut rect = RECT::default();
            let width = if unsafe { GetWindowRect(hwnd, &mut rect) }.is_ok() {
                rect.right - rect.left
            } else {
                0
            };
            if best.is_none_or(|(have, _)| width > have) {
                *best = Some((width, hwnd));
            }
        }
        walk_filename_edit(hwnd, best);
        after = Some(hwnd);
    }
}

fn find_shell_view(root: HWND) -> Option<HWND> {
    if root.0.is_null() {
        return None;
    }
    find_child(root, "SHELLDLL_DefView")
}

fn find_child(parent: HWND, class: &str) -> Option<HWND> {
    let mut after: Option<HWND> = None;
    loop {
        let Ok(hwnd) = (unsafe { FindWindowExW(Some(parent), after, PCWSTR::null(), PCWSTR::null()) })
        else {
            return None;
        };
        if class_name(hwnd) == class {
            return Some(hwnd);
        }
        if let Some(found) = find_child(hwnd, class) {
            return Some(found);
        }
        after = Some(hwnd);
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 64];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if length <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buffer[..length as usize])
    }
}

fn window_title(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    if length <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buffer[..length as usize])
    }
}

fn is_save_title(title: &str) -> bool {
    let title = title.trim();
    if title.is_empty() {
        return false;
    }
    if title.contains("另存为") || title.to_ascii_lowercase().contains("save as") {
        return true;
    }
    is_open_caption(title)
}

/// File Open uses the same common dialog as Save As, with a different caption.
fn is_open_caption(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    if lower == "open" || lower == "open file" || lower == "browse" || lower == "select file" {
        return true;
    }
    let Some(rest) = title.strip_prefix("打开") else {
        return false;
    };
    rest.chars().count() <= 6
}

fn select_view_item(view: &IFolderView, index: i32) -> bool {
    let mut before = POINT::default();
    let saved = unsafe { GetCursorPos(&mut before) }.is_ok();
    let foreground = unsafe { GetForegroundWindow() };
    let flags = SVSI_KEYBOARDSELECT.0
        | SVSI_DESELECTOTHERS.0
        | SVSI_ENSUREVISIBLE.0
        | SVSI_NOTAKEFOCUS.0;
    let selected = unsafe { view.SelectItem(index, flags as u32) }.is_ok();
    if saved {
        let mut now = POINT::default();
        if unsafe { GetCursorPos(&mut now) }.is_ok() && (now.x != before.x || now.y != before.y) {
            let _ = unsafe { SetCursorPos(before.x, before.y) };
        }
    }
    let after = unsafe { GetForegroundWindow() };
    if !foreground.0.is_null() && after != foreground {
        let _ = unsafe { SetForegroundWindow(foreground) };
    }
    selected
}

fn active_folder() -> Option<(IFolderView, String)> {
    ensure_com();
    let pinned = PIN.load(Ordering::Relaxed);
    let foreground = if pinned != 0 {
        windows::Win32::Foundation::HWND(pinned as *mut std::ffi::c_void)
    } else {
        unsafe { GetForegroundWindow() }
    };
    if foreground.0.is_null() {
        return None;
    }
    shell_folder(foreground)
}

fn shell_folder(foreground: windows::Win32::Foundation::HWND) -> Option<(IFolderView, String)> {
    let shell: IShellWindows = unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) }.ok()?;
    let count = unsafe { shell.Count() }.ok()?;
    for index in 0..count {
        let Ok(dispatch) = (unsafe { shell.Item(&variant_i4(index)) }) else {
            continue;
        };
        let Ok(browser) = dispatch.cast::<IWebBrowser2>() else {
            continue;
        };
        let Ok(app) = browser.cast::<IWebBrowserApp>() else {
            continue;
        };
        let Ok(handle) = (unsafe { app.HWND() }) else {
            continue;
        };
        if handle.0 != foreground.0 as isize {
            continue;
        }
        let service: IServiceProvider = browser.cast().ok()?;
        let shell_browser: IShellBrowser =
            unsafe { service.QueryService(&SID_STopLevelBrowser) }.ok()?;
        let shell_view: IShellView = unsafe { shell_browser.QueryActiveShellView() }.ok()?;
        let view: IFolderView = shell_view.cast().ok()?;
        let path = folder_path(&view).unwrap_or_default();
        return Some((view, path));
    }
    None
}

fn folder_path(view: &IFolderView) -> Option<String> {
    let persist: IPersistFolder2 = unsafe { view.GetFolder() }.ok()?;
    let pidl = unsafe { persist.GetCurFolder() }.ok()?;
    let path = unsafe {
        let name = SHGetNameFromIDList(pidl, SIGDN_FILESYSPATH).ok();
        let text = name.and_then(|name| {
            let text = name.to_string().ok();
            CoTaskMemFree(Some(name.0 as *const _));
            text
        });
        CoTaskMemFree(Some(pidl as *const _));
        text
    };
    path
}

fn find_index(view: &IFolderView, folder: &str, query: &str) -> Option<i32> {
    with_names(view, folder, |names| {
        if let Some(index) = names
            .iter()
            .find(|(_, name)| starts_with_ignore(name, query))
            .map(|(index, _)| *index)
        {
            return Some(index);
        }
        names
            .iter()
            .find(|(_, name)| contains_ignore(name, query))
            .map(|(index, _)| *index)
    })
}

fn with_names<T>(view: &IFolderView, folder: &str, body: impl FnOnce(&[(i32, String)]) -> T) -> T {
    let pinned = PIN.load(Ordering::Relaxed);
    let hwnd = if pinned != 0 {
        pinned
    } else {
        unsafe { GetForegroundWindow() }.0 as isize
    };
    let key = format!("{hwnd}:{folder}");
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.key != key {
            cache.names = list_names(view);
            cache.key = key;
        }
        body(&cache.names)
    })
}

fn list_names(view: &IFolderView) -> Vec<(i32, String)> {
    let count = unsafe { view.ItemCount(SVGIO_ALLVIEW) }.unwrap_or(0).min(NAME_LIMIT);
    let Ok(folder) = (unsafe { view.GetFolder::<IShellFolder>() }) else {
        return Vec::new();
    };
    let mut names = Vec::with_capacity(count as usize);
    for index in 0..count {
        let Ok(pidl) = (unsafe { view.Item(index) }) else {
            continue;
        };
        if let Some(name) = display_name(&folder, pidl) {
            names.push((index, name));
        }
        unsafe { CoTaskMemFree(Some(pidl as *const _)) };
    }
    names
}

fn display_name(folder: &IShellFolder, pidl: *mut ITEMIDLIST) -> Option<String> {
    let mut strret = STRRET::default();
    unsafe { folder.GetDisplayNameOf(pidl, SHGDN_INFOLDER, &mut strret) }.ok()?;
    let mut buffer = [0u16; 520];
    unsafe { StrRetToBufW(&mut strret, Some(pidl), &mut buffer) }.ok()?;
    let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
    let name = String::from_utf16_lossy(&buffer[..end]);
    if name.is_empty() { None } else { Some(name) }
}

fn variant_i4(value: i32) -> VARIANT {
    unsafe {
        let mut variant = VARIANT::default();
        let header = &mut *variant.Anonymous.Anonymous;
        header.vt = VT_I4;
        header.Anonymous.lVal = value;
        variant
    }
}

fn starts_with_ignore(name: &str, query: &str) -> bool {
    let name = name.to_lowercase();
    let query = query.to_lowercase();
    name.starts_with(&query)
}

fn contains_ignore(name: &str, query: &str) -> bool {
    let name = name.to_lowercase();
    let query = query.to_lowercase();
    name.contains(&query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_dialog_uses_the_save_bar() {
        assert!(is_save_title("另存为"));
        assert!(is_save_title("Save As"));
        assert!(is_save_title("打开"));
        assert!(is_save_title("打开文件"));
        assert!(is_save_title("Open"));
        assert!(is_save_title("Open File"));
        assert!(!is_save_title("打开了三个很重要的文件夹"));
        assert!(!is_save_title("Cabinet"));
    }

    #[test]
    fn chinese_names_match_as_whole_characters() {
        assert!(starts_with_ignore("龚工.zip", "龚"));
        assert!(contains_ignore("报告龚工.docx", "龚工"));
        assert!(starts_with_ignore("GongGong.txt", "gong"));
        assert!(!contains_ignore("abc", "龚"));
    }
}
